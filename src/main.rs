//! Mizan Connect — entrypoint.
//!
//! Wiring only. Anything substantive lives in the library crate.

use anyhow::Context;
use mizan_connect::{
    auth::JwksCache, config::Config, db, server, shutdown, state::AppState, telemetry,
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

    let state = AppState::new(config.clone(), pool, jwks);
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
