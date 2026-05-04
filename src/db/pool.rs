//! Postgres connection pool helpers.

use std::time::Duration;

use secrecy::ExposeSecret;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::config::Config;

/// Open a connection pool against `config.database_url`. Verifies a single
/// connection succeeds before returning so callers fail fast.
pub async fn connect(config: &Config) -> Result<PgPool, sqlx::Error> {
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .min_connections(1)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Some(Duration::from_secs(60 * 10)))
        .max_lifetime(Some(Duration::from_secs(60 * 30)))
        .connect(config.database_url.expose_secret())
        .await?;

    // Smoke ping: round-trip a `SELECT 1` to surface bad creds early.
    sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await?;

    Ok(pool)
}

/// Apply embedded migrations from `./migrations`.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
