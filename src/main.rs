//! Mizan Connect — entrypoint.
//!
//! Wiring only. Anything substantive lives in the library crate.

use anyhow::Context;
use mizan_connect::{
    auth::JwksCache,
    billing::{BillingContext, StripeClient, StripePrices},
    config::Config,
    db, server, shutdown,
    snaptrade::{client::SnaptradeClient, encryption::EncryptionKey},
    state::AppState,
    telemetry,
};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::load().context("loading configuration from environment")?;
    let _sentry_guard = telemetry::init(&config);

    tracing::info!(
        host = %config.app_host,
        port = config.app_port,
        env = ?config.app_env,
        "starting mizan-connect"
    );

    let pool = db::connect(&config)
        .await
        .context("connecting to Postgres")?;
    db::run_migrations(&pool)
        .await
        .context("running database migrations")?;

    let jwks = JwksCache::new(config.jwks_url());
    jwks.warm().await;
    let _refresher = jwks.spawn_refresher();

    // Self-test the broker-secret encryption key before binding a port —
    // a misconfigured key here would silently corrupt every SnapTrade row.
    let encryption = EncryptionKey::from_bytes(&config.snaptrade.broker_secret_encryption_key)
        .context("MIZAN_BROKER_SECRET_ENCRYPTION_KEY length validation")?;
    encryption
        .self_test()
        .context("AES-256-GCM self-test failed (broker secret encryption key is unusable)")?;
    tracing::info!("AES-256-GCM self-test passed");

    let snaptrade = SnaptradeClient::new(
        config.snaptrade.api_base.clone(),
        config.snaptrade.client_id.clone(),
        config.snaptrade.consumer_key.clone(),
    )
    .context("constructing SnaptradeClient")?;

    // Billing context (Chunk 4). Optional: `None` collapses the billing
    // endpoints to `not_implemented` so dev/staging without Stripe still boots.
    let billing = config.billing.as_ref().map(|bc| {
        let stripe = StripeClient::new(bc.stripe_secret_key.clone());
        let prices = StripePrices {
            basic_monthly: bc.stripe_prices.basic_monthly.clone(),
            basic_yearly: bc.stripe_prices.basic_yearly.clone(),
            pro_monthly: bc.stripe_prices.pro_monthly.clone(),
            pro_yearly: bc.stripe_prices.pro_yearly.clone(),
            enterprise_monthly: bc.stripe_prices.enterprise_monthly.clone(),
            enterprise_yearly: bc.stripe_prices.enterprise_yearly.clone(),
        };
        BillingContext::new(
            stripe,
            prices,
            bc.stripe_webhook_secret.clone(),
            bc.return_url.clone(),
            bc.openai_api_key.clone(),
        )
    });
    if billing.is_some() {
        tracing::info!("billing context loaded (Stripe configured)");
    } else {
        tracing::warn!("billing context not configured — checkout/portal endpoints return 501");
    }

    let state = AppState::new(config.clone(), pool, jwks, snaptrade, encryption, billing);
    let app = server::build_app(state);
    let addr = server::bind_addr(&config);

    let listener = TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;
    tracing::info!(local_addr = %listener.local_addr()?, "listening");

    serve_with_deadline(listener, app).await?;
    tracing::info!("clean shutdown");
    Ok(())
}

/// Run the server until shutdown, capping the drain phase at
/// [`shutdown::DRAIN_DEADLINE`].
async fn serve_with_deadline(listener: TcpListener, app: axum::Router) -> anyhow::Result<()> {
    let signal = std::sync::Arc::new(tokio::sync::Notify::new());
    let signal_listener = signal.clone();
    let signal_self = signal.clone();

    tokio::spawn(async move {
        shutdown::wait_for_signal().await;
        signal_self.notify_waiters();
    });

    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        signal_listener.notified().await;
    });

    // Spawn the serve future so we can race the drain deadline against it.
    let serve_handle = tokio::spawn(async move { serve.await });

    // Wait until shutdown is signalled.
    signal.notified().await;

    // Now bound the drain.
    match tokio::time::timeout(shutdown::DRAIN_DEADLINE, serve_handle).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(err))) => Err(err).context("axum::serve"),
        Ok(Err(err)) => Err(err).context("server task panicked"),
        Err(_) => {
            tracing::error!(
                "drain deadline of {:?} exceeded; in-flight requests will be dropped",
                shutdown::DRAIN_DEADLINE
            );
            Ok(())
        }
    }
}
