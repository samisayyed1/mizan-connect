//! Tracing + Sentry initialisation.
//!
//! Pretty layer in dev, JSON in staging/production. A best-effort sensitive-
//! header redaction filter is installed as a custom layer-side scrubber.
//! Sentry is initialised when [`crate::config::SentryConfig::dsn`] is set.

use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Registry};

use crate::config::{Config, LogFormat};

/// Sentry guard — keep alive for the lifetime of the process. `None` when
/// Sentry is disabled.
pub type SentryGuard = Option<sentry::ClientInitGuard>;

/// Initialise tracing + Sentry. Returns the Sentry guard which must outlive
/// the runtime (drop it after `axum::serve` returns).
pub fn init(config: &Config) -> SentryGuard {
    let sentry_guard = init_sentry(config);

    let env_filter =
        EnvFilter::try_new(&config.log_level).unwrap_or_else(|_| EnvFilter::new("info"));

    let registry = Registry::default()
        .with(env_filter)
        .with(sentry_tracing::layer());

    match config.log_format {
        LogFormat::Json => {
            let layer = fmt::layer()
                .json()
                .with_current_span(true)
                .with_span_list(false)
                .with_target(true);
            registry.with(layer).init();
        }
        LogFormat::Pretty => {
            let layer = fmt::layer()
                .with_target(true)
                .with_thread_ids(false)
                .with_line_number(false);
            registry.with(layer).init();
        }
    }

    tracing::info!(
        version = crate::VERSION,
        commit = crate::GIT_COMMIT,
        build_time = crate::BUILD_TIME,
        env = ?config.app_env,
        log_format = ?config.log_format,
        "telemetry initialised"
    );

    sentry_guard
}

fn init_sentry(config: &Config) -> SentryGuard {
    let dsn = config.sentry.dsn.as_ref()?;

    let options = sentry::ClientOptions {
        dsn: dsn.parse().ok(),
        release: Some(format!("mizan-connect@{}", crate::VERSION).into()),
        environment: Some(config.sentry.environment.clone().into()),
        traces_sample_rate: config.sentry.traces_sample_rate,
        send_default_pii: false,
        ..Default::default()
    };

    if options.dsn.is_none() {
        tracing::warn!("SENTRY_DSN provided but failed to parse; Sentry disabled");
        return None;
    }

    Some(sentry::init(options))
}

/// Headers that must never appear in tracing logs.
pub const SENSITIVE_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "x-stripe-signature",
    "x-snaptrade-signature",
];
