//! Application configuration loaded from environment / `.env`.
//!
//! Validated at startup via [`Config::load`]. Any validation failure aborts
//! the process before binding a port — fail fast.

use std::str::FromStr;

use figment::providers::Env;
use figment::Figment;
use secrecy::SecretString;
use serde::Deserialize;

/// Operating environment for the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppEnv {
    Development,
    Staging,
    Production,
    Test,
}

impl AppEnv {
    pub fn is_production(self) -> bool {
        matches!(self, AppEnv::Production)
    }
    pub fn is_test(self) -> bool {
        matches!(self, AppEnv::Test)
    }
}

impl FromStr for AppEnv {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "development" | "dev" => Ok(AppEnv::Development),
            "staging" | "stage" => Ok(AppEnv::Staging),
            "production" | "prod" => Ok(AppEnv::Production),
            "test" => Ok(AppEnv::Test),
            other => Err(format!("unknown APP_ENV value: {other}")),
        }
    }
}

/// Logging output format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    Pretty,
    Json,
}

impl FromStr for LogFormat {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pretty" => Ok(LogFormat::Pretty),
            "json" => Ok(LogFormat::Json),
            other => Err(format!("unknown LOG_FORMAT value: {other}")),
        }
    }
}

/// Sentry-related options.
#[derive(Debug, Clone)]
pub struct SentryConfig {
    pub dsn: Option<String>,
    pub environment: String,
    pub traces_sample_rate: f32,
}

/// Application configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub app_host: String,
    pub app_port: u16,
    pub app_env: AppEnv,

    pub log_level: String,
    pub log_format: LogFormat,

    pub database_url: SecretString,
    pub database_max_connections: u32,

    pub supabase_url: String,
    pub supabase_jwt_audience: String,
    pub supabase_service_role_key: Option<SecretString>,

    pub cors_allowed_origins: Vec<String>,
    pub rate_limit_per_minute: u32,

    pub sentry: SentryConfig,

    /// HS256 secret accepted by the auth layer in non-production builds.
    /// Always `None` when `app_env == Production`, regardless of env value.
    pub test_jwt_secret: Option<SecretString>,

    /// SnapTrade integration (Chunk 3). Required outside of test env.
    pub snaptrade: SnaptradeConfig,
}

/// SnapTrade configuration. Required outside of `APP_ENV=test`.
#[derive(Debug, Clone)]
pub struct SnaptradeConfig {
    pub client_id: String,
    pub consumer_key: SecretString,
    pub api_base: String,
    pub redirect_uri: String,
    /// AES-256-GCM key bytes — exactly 32 bytes after base64 decode.
    pub broker_secret_encryption_key: Vec<u8>,
    /// HS256 signing key for callback state JWTs — ≥ 32 bytes.
    pub state_secret: SecretString,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required env var: {0}")]
    Missing(&'static str),
    #[error("invalid value for {var}: {reason}")]
    Invalid { var: &'static str, reason: String },
    #[error("CORS allowlist may not contain '*' when auth is enabled")]
    CorsWildcardWithAuth,
    #[error("MIZAN_BROKER_SECRET_ENCRYPTION_KEY must decode to exactly 32 bytes (got {0})")]
    BadEncryptionKeyLength(usize),
    #[error("MIZAN_SNAPTRADE_STATE_SECRET must decode to >= 32 bytes (got {0})")]
    StateSecretTooShort(usize),
    #[error("figment: {0}")]
    Figment(#[from] figment::Error),
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    app_host: Option<String>,
    app_port: Option<u16>,
    app_env: Option<String>,

    log_level: Option<String>,
    log_format: Option<String>,

    database_url: String,
    database_max_connections: Option<u32>,

    supabase_url: String,
    supabase_jwt_audience: Option<String>,
    supabase_service_role_key: Option<String>,

    mizan_cors_allowed_origins: Option<String>,
    rate_limit_per_minute: Option<u32>,

    sentry_dsn: Option<String>,
    sentry_environment: Option<String>,
    sentry_traces_sample_rate: Option<f32>,

    mizan_test_jwt_secret: Option<String>,

    // SnapTrade (Chunk 3)
    snaptrade_client_id: Option<String>,
    snaptrade_consumer_key: Option<String>,
    snaptrade_api_base: Option<String>,
    snaptrade_redirect_uri: Option<String>,
    mizan_broker_secret_encryption_key: Option<String>,
    mizan_snaptrade_state_secret: Option<String>,
}

impl Config {
    /// Load and validate configuration from environment variables.
    ///
    /// Reads `.env` first (no override of already-set env), then merges
    /// process environment. Returns [`ConfigError`] on any validation failure
    /// so the caller can surface a clean startup error.
    pub fn load() -> Result<Self, ConfigError> {
        // Best-effort .env load; missing file is fine.
        let _ = dotenvy::dotenv();

        let raw: RawConfig = Figment::new().merge(Env::raw().lowercase(true)).extract()?;

        let app_env = parse_required("APP_ENV", raw.app_env.as_deref(), Some("development"))?;

        let log_format = parse_optional::<LogFormat>(
            "LOG_FORMAT",
            raw.log_format.as_deref(),
            match app_env {
                AppEnv::Production | AppEnv::Staging => LogFormat::Json,
                _ => LogFormat::Pretty,
            },
        )?;

        let database_url = trim_required(&raw.database_url, "DATABASE_URL")?;
        let supabase_url = trim_required(&raw.supabase_url, "SUPABASE_URL")?;

        if !supabase_url.starts_with("https://") && !app_env.is_test() {
            return Err(ConfigError::Invalid {
                var: "SUPABASE_URL",
                reason: "must use https:// scheme outside of test env".to_string(),
            });
        }

        let cors_allowed_origins = parse_cors(
            raw.mizan_cors_allowed_origins.as_deref(),
            // In Chunk 1 every endpoint behind /v1 requires auth. Wildcard
            // CORS is therefore always disallowed in non-test envs.
            !app_env.is_test(),
        )?;

        // Build SnapTrade config first — it borrows `&raw` and would
        // otherwise conflict with the `raw.…` field moves below.
        let snaptrade = build_snaptrade_config(&raw, app_env)?;

        let test_jwt_secret = if app_env.is_production() {
            None
        } else {
            raw.mizan_test_jwt_secret
                .filter(|s| !s.trim().is_empty())
                .map(SecretString::from)
        };

        let service_role_key = raw
            .supabase_service_role_key
            .filter(|s| !s.trim().is_empty())
            .map(SecretString::from);

        let sentry = SentryConfig {
            dsn: raw.sentry_dsn.filter(|s| !s.trim().is_empty()),
            environment: raw.sentry_environment.unwrap_or_else(|| match app_env {
                AppEnv::Production => "production".into(),
                AppEnv::Staging => "staging".into(),
                AppEnv::Development => "development".into(),
                AppEnv::Test => "test".into(),
            }),
            traces_sample_rate: raw.sentry_traces_sample_rate.unwrap_or(0.1).clamp(0.0, 1.0),
        };

        Ok(Self {
            app_host: raw.app_host.unwrap_or_else(|| "0.0.0.0".into()),
            app_port: raw.app_port.unwrap_or(8080),
            app_env,
            log_level: raw.log_level.unwrap_or_else(|| "info".into()),
            log_format,
            database_url: SecretString::from(database_url),
            database_max_connections: raw.database_max_connections.unwrap_or(10).max(1),
            supabase_url,
            supabase_jwt_audience: raw
                .supabase_jwt_audience
                .unwrap_or_else(|| "authenticated".into()),
            supabase_service_role_key: service_role_key,
            cors_allowed_origins,
            rate_limit_per_minute: raw.rate_limit_per_minute.unwrap_or(100).max(1),
            sentry,
            test_jwt_secret,
            snaptrade,
        })
    }

    /// JWKS URL derived from `SUPABASE_URL`.
    pub fn jwks_url(&self) -> String {
        format!(
            "{}/auth/v1/.well-known/jwks.json",
            self.supabase_url.trim_end_matches('/')
        )
    }

    /// Expected JWT issuer (`iss`) claim.
    pub fn supabase_jwt_issuer(&self) -> String {
        format!("{}/auth/v1", self.supabase_url.trim_end_matches('/'))
    }
}

fn build_snaptrade_config(
    raw: &RawConfig,
    app_env: AppEnv,
) -> Result<SnaptradeConfig, ConfigError> {
    use base64::Engine;

    let required = !app_env.is_test();
    let api_base = raw
        .snaptrade_api_base
        .clone()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://api.snaptrade.com/api/v1".to_string());

    let client_id = pick_required(
        "SNAPTRADE_CLIENT_ID",
        raw.snaptrade_client_id.as_deref(),
        required,
    )?
    .unwrap_or_default();
    let consumer_key_str = pick_required(
        "SNAPTRADE_CONSUMER_KEY",
        raw.snaptrade_consumer_key.as_deref(),
        required,
    )?
    .unwrap_or_default();
    let redirect_uri = pick_required(
        "SNAPTRADE_REDIRECT_URI",
        raw.snaptrade_redirect_uri.as_deref(),
        required,
    )?
    .unwrap_or_default();

    // Encryption key — base64 → exactly 32 bytes.
    let enc_key_b64 = pick_required(
        "MIZAN_BROKER_SECRET_ENCRYPTION_KEY",
        raw.mizan_broker_secret_encryption_key.as_deref(),
        required,
    )?;
    let broker_secret_encryption_key = match enc_key_b64 {
        Some(s) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(s.trim())
                .map_err(|e| ConfigError::Invalid {
                    var: "MIZAN_BROKER_SECRET_ENCRYPTION_KEY",
                    reason: format!("base64 decode: {e}"),
                })?;
            if bytes.len() != 32 {
                return Err(ConfigError::BadEncryptionKeyLength(bytes.len()));
            }
            bytes
        }
        None => Vec::new(), // test env without key — encryption is opt-in
    };

    // State secret — base64 → >= 32 bytes.
    let state_b64 = pick_required(
        "MIZAN_SNAPTRADE_STATE_SECRET",
        raw.mizan_snaptrade_state_secret.as_deref(),
        required,
    )?;
    let state_secret = match state_b64 {
        Some(s) => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(s.trim())
                .map_err(|e| ConfigError::Invalid {
                    var: "MIZAN_SNAPTRADE_STATE_SECRET",
                    reason: format!("base64 decode: {e}"),
                })?;
            if bytes.len() < 32 {
                return Err(ConfigError::StateSecretTooShort(bytes.len()));
            }
            // jsonwebtoken's HS256 takes the raw bytes — keep the raw
            // string so the secret survives across processes regardless
            // of base64 padding canonicalization.
            SecretString::from(s)
        }
        None => SecretString::from(String::new()),
    };

    Ok(SnaptradeConfig {
        client_id,
        consumer_key: SecretString::from(consumer_key_str),
        api_base,
        redirect_uri,
        broker_secret_encryption_key,
        state_secret,
    })
}

/// `pick_required(var, raw, required)`:
/// - if `raw` is non-empty → `Ok(Some(value))`
/// - if `raw` is empty AND `required` → `Err(Missing)`
/// - otherwise → `Ok(None)`
fn pick_required(
    var: &'static str,
    raw: Option<&str>,
    required: bool,
) -> Result<Option<String>, ConfigError> {
    let value = raw.map(str::trim).filter(|s| !s.is_empty());
    match value {
        Some(v) => Ok(Some(v.to_string())),
        None if required => Err(ConfigError::Missing(var)),
        None => Ok(None),
    }
}

fn trim_required(value: &str, var: &'static str) -> Result<String, ConfigError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(ConfigError::Missing(var));
    }
    Ok(trimmed.to_string())
}

fn parse_required<T: FromStr<Err = String>>(
    var: &'static str,
    raw: Option<&str>,
    default: Option<&str>,
) -> Result<T, ConfigError> {
    let value = match raw.filter(|s| !s.trim().is_empty()) {
        Some(v) => v,
        None => default.ok_or(ConfigError::Missing(var))?,
    };
    T::from_str(value).map_err(|reason| ConfigError::Invalid { var, reason })
}

fn parse_optional<T: FromStr<Err = String>>(
    var: &'static str,
    raw: Option<&str>,
    default: T,
) -> Result<T, ConfigError> {
    match raw.filter(|s| !s.trim().is_empty()) {
        Some(v) => T::from_str(v).map_err(|reason| ConfigError::Invalid { var, reason }),
        None => Ok(default),
    }
}

fn parse_cors(raw: Option<&str>, reject_wildcard: bool) -> Result<Vec<String>, ConfigError> {
    let raw = raw.unwrap_or("");
    let origins: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if reject_wildcard && origins.iter().any(|o| o == "*") {
        return Err(ConfigError::CorsWildcardWithAuth);
    }

    Ok(origins)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn cors_rejects_wildcard_when_strict() {
        let err = parse_cors(Some("*,https://example.com"), true).unwrap_err();
        assert!(matches!(err, ConfigError::CorsWildcardWithAuth));
    }

    #[test]
    fn cors_allows_wildcard_in_test_env() {
        let origins = parse_cors(Some("*"), false).expect("wildcard allowed in test");
        assert_eq!(origins, vec!["*".to_string()]);
    }

    #[test]
    fn cors_parses_csv() {
        let origins =
            parse_cors(Some("https://a.test, https://b.test , "), true).expect("valid origins");
        assert_eq!(
            origins,
            vec!["https://a.test".to_string(), "https://b.test".to_string()]
        );
    }

    #[test]
    fn app_env_parses_known() {
        assert_eq!(AppEnv::from_str("production"), Ok(AppEnv::Production));
        assert_eq!(AppEnv::from_str("DEV"), Ok(AppEnv::Development));
        assert!(AppEnv::from_str("nope").is_err());
    }
}
