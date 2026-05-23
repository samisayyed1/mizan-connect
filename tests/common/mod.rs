//! Shared test harness.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//!
//! Each `TestApp::spawn` brings up a fresh Postgres container, runs
//! migrations, configures the app with a deterministic test JWT secret,
//! binds a server on a random port, and returns a ready-to-call client.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use mizan_connect::auth::JwksCache;
use mizan_connect::config::{AppEnv, Config, LogFormat, SentryConfig, SnaptradeConfig};
use mizan_connect::server::build_app;
use mizan_connect::snaptrade::client::SnaptradeClient;
use mizan_connect::snaptrade::encryption::EncryptionKey;
use mizan_connect::state::AppState;
use reqwest::Client;
use secrecy::SecretString;
use serde_json::json;
use sqlx::PgPool;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::runners::AsyncRunner;
use testcontainers_modules::testcontainers::ContainerAsync;
use time::OffsetDateTime;

pub const TEST_JWT_SECRET: &str = "mizan-test-secret-do-not-ship-please";
pub const TEST_STATE_SECRET: &str = "mizan-snaptrade-state-secret-32+bytes-at-least-thanks";
pub const TEST_ENCRYPTION_KEY: [u8; 32] = [42u8; 32];
pub const TEST_SNAPTRADE_CLIENT_ID: &str = "MIZAN-TEST";
pub const TEST_SNAPTRADE_CONSUMER_KEY: &str = "test-consumer-key-do-not-rotate";
const TEST_ISSUER: &str = "https://test.supabase.co/auth/v1";

/// Live test server bound to a random port on `127.0.0.1`.
pub struct TestApp {
    pub address: String,
    pub client: Client,
    pub pool: PgPool,
    /// Held to keep the Postgres container alive for the test's lifetime.
    _container: Arc<ContainerAsync<Postgres>>,
}

impl TestApp {
    /// Spawn with the production SnapTrade base URL. Use this when no test
    /// will exercise the SnapTrade client (existing health/auth/connect tests).
    pub async fn spawn() -> Self {
        Self::spawn_with_snaptrade("https://api.snaptrade.invalid/api/v1").await
    }

    /// Spawn with a caller-provided SnapTrade base URL — for snaptrade
    /// integration tests that point at a wiremock server.
    pub async fn spawn_with_snaptrade(snaptrade_api_base: &str) -> Self {
        let _ = tracing_subscriber::fmt()
            .with_test_writer()
            .with_max_level(tracing::Level::WARN)
            .try_init();

        let container = Postgres::default()
            .start()
            .await
            .expect("postgres container should start");
        let host = container
            .get_host()
            .await
            .expect("postgres host")
            .to_string();
        let port = container
            .get_host_port_ipv4(5432)
            .await
            .expect("postgres port");
        let database_url = format!("postgres://postgres:postgres@{host}:{port}/postgres");

        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(10))
            .connect(&database_url)
            .await
            .expect("pool");

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("migrations should apply");

        let config = Config {
            app_host: "127.0.0.1".into(),
            app_port: 0,
            app_env: AppEnv::Test,
            log_level: "warn".into(),
            log_format: LogFormat::Pretty,
            database_url: SecretString::from(database_url.clone()),
            database_max_connections: 5,
            supabase_url: "https://test.supabase.co".into(),
            supabase_jwt_audience: "authenticated".into(),
            supabase_service_role_key: None,
            cors_allowed_origins: vec!["http://localhost:1420".into()],
            rate_limit_per_minute: 600,
            sentry: SentryConfig {
                dsn: None,
                environment: "test".into(),
                traces_sample_rate: 0.0,
            },
            test_jwt_secret: Some(SecretString::from(String::from(TEST_JWT_SECRET))),
            snaptrade: SnaptradeConfig {
                client_id: TEST_SNAPTRADE_CLIENT_ID.into(),
                consumer_key: SecretString::from(String::from(TEST_SNAPTRADE_CONSUMER_KEY)),
                api_base: snaptrade_api_base.to_string(),
                redirect_uri: "http://127.0.0.1/api/v1/sync/snaptrade/callback".into(),
                broker_secret_encryption_key: TEST_ENCRYPTION_KEY.to_vec(),
                state_secret: SecretString::from(String::from(TEST_STATE_SECRET)),
            },
            // Billing left unconfigured by default — tests that need Stripe
            // wire a `BillingContext` directly into `AppState`.
            billing: None,
        };

        let jwks = JwksCache::new(config.jwks_url());
        // Don't warm — test path uses HS256 fallback so JWKS is unreachable.
        let snaptrade = SnaptradeClient::new(
            snaptrade_api_base.to_string(),
            TEST_SNAPTRADE_CLIENT_ID,
            SecretString::from(String::from(TEST_SNAPTRADE_CONSUMER_KEY)),
        )
        .expect("SnaptradeClient");
        let encryption = EncryptionKey::from_bytes(&TEST_ENCRYPTION_KEY).expect("32-byte test key");

        // No billing context in the default harness — tests that need
        // billing wire one explicitly via `TestApp::spawn_with_billing`.
        let state = AppState::new(
            config.clone(),
            pool.clone(),
            jwks,
            snaptrade,
            encryption,
            None,
        );
        let app = build_app(state);

        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind");
        let address = format!("http://{}", listener.local_addr().expect("local_addr"));

        tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await;
        });

        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("client");

        Self {
            address,
            client,
            pool,
            _container: Arc::new(container),
        }
    }

    /// Mint a test JWT signed with the harness's HS256 secret.
    pub fn mint_jwt(
        &self,
        sub: &str,
        email: Option<&str>,
        display_name: Option<&str>,
        ttl: Duration,
    ) -> String {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let exp = now + ttl.as_secs() as i64;

        let mut user_metadata = serde_json::Map::new();
        if let Some(name) = display_name {
            user_metadata.insert("display_name".into(), json!(name));
        }

        let claims = json!({
            "sub": sub,
            "email": email,
            "iss": TEST_ISSUER,
            "aud": "authenticated",
            "exp": exp,
            "iat": now,
            "user_metadata": user_metadata,
            "app_metadata": {},
            "role": "authenticated",
        });

        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
        )
        .expect("encode test jwt")
    }

    /// Mint an expired JWT for testing rejection.
    pub fn mint_expired_jwt(&self, sub: &str) -> String {
        let now = OffsetDateTime::now_utc().unix_timestamp();
        let claims = json!({
            "sub": sub,
            "email": "expired@example.com",
            "iss": TEST_ISSUER,
            "aud": "authenticated",
            "exp": now - 3600,
            "iat": now - 7200,
            "user_metadata": {},
            "app_metadata": {},
        });
        encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(TEST_JWT_SECRET.as_bytes()),
        )
        .expect("encode test jwt")
    }
}
