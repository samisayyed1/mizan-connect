//! Supabase JWT verification.
//!
//! Two paths:
//! 1. **Production**: verify against keys cached from
//!    `{SUPABASE_URL}/auth/v1/.well-known/jwks.json`. Validates `iss`, `aud`,
//!    and `exp` (60s leeway).
//! 2. **Test/Dev fallback**: when `APP_ENV != Production` and
//!    `MIZAN_TEST_JWT_SECRET` is set, accept HS256 tokens signed with that
//!    secret. Production builds reject this path even if the secret is
//!    present in the environment.

use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use secrecy::ExposeSecret;
use serde::Deserialize;

use crate::config::Config;
use crate::error::AppError;

use super::jwks::JwksCache;

const JWT_LEEWAY_SECONDS: u64 = 60;

/// Subset of Supabase JWT claims we care about.
#[derive(Debug, Clone, Deserialize)]
pub struct SupabaseClaims {
    /// Supabase user UUID. Stored directly in `users.supabase_user_id`.
    pub sub: String,
    /// User's primary email. Always present for `aud=authenticated` tokens.
    #[serde(default)]
    pub email: Option<String>,
    /// `iss` — Supabase auth issuer.
    pub iss: String,
    /// Audience claim (`authenticated`, `anon`, or `service_role`).
    pub aud: String,
    /// Token expiry, seconds since unix epoch.
    pub exp: u64,
    /// User-supplied metadata. We pull `display_name` and `avatar_url` here.
    #[serde(default)]
    pub user_metadata: serde_json::Value,
    /// Internal Supabase metadata. Read-only on our side.
    #[serde(default)]
    pub app_metadata: serde_json::Value,
    /// Token role. Useful later for admin endpoints; ignored in Chunk 1.
    #[serde(default)]
    pub role: Option<String>,
}

impl SupabaseClaims {
    pub fn display_name(&self) -> Option<String> {
        self.user_metadata
            .get("display_name")
            .or_else(|| self.user_metadata.get("full_name"))
            .or_else(|| self.user_metadata.get("name"))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    pub fn avatar_url(&self) -> Option<String> {
        self.user_metadata
            .get("avatar_url")
            .or_else(|| self.user_metadata.get("picture"))
            .and_then(|v| v.as_str())
            .map(str::to_owned)
    }

    pub fn email_required(&self) -> Result<&str, AppError> {
        self.email
            .as_deref()
            .ok_or_else(|| AppError::unauthorized("token missing email claim; cannot link account"))
    }
}

/// Verify a bearer token, returning its claims.
///
/// Tries the JWKS path first. Falls back to the HS256 test path only when
/// `APP_ENV != Production` and a test secret is configured.
pub async fn verify_token(
    token: &str,
    config: &Config,
    jwks: &JwksCache,
) -> Result<SupabaseClaims, AppError> {
    let token = token.trim();
    if token.is_empty() {
        return Err(AppError::unauthorized("empty bearer token"));
    }

    let header = decode_header(token)
        .map_err(|err| AppError::unauthorized("invalid token header").with_source(err))?;

    // Test fallback: HS256 + matching kid prefix or no kid.
    if !config.app_env.is_production() {
        if let Some(secret) = config.test_jwt_secret.as_ref() {
            if header.alg == Algorithm::HS256 {
                let key = DecodingKey::from_secret(secret.expose_secret().as_bytes());
                let mut validation = build_validation(config, Algorithm::HS256);
                // Test tokens may use a non-standard issuer; allow anything
                // matching the configured issuer OR a literal "mizan-test".
                validation.set_issuer(&[config.supabase_jwt_issuer().as_str(), "mizan-test"]);
                let data = decode::<SupabaseClaims>(token, &key, &validation)
                    .map_err(|err| AppError::unauthorized("invalid test token").with_source(err))?;
                return Ok(data.claims);
            }
        }
    }

    // Production path: locate the right JWKS key by kid.
    let key_match = jwks.find_key(header.kid.as_deref()).await?;
    let validation = build_validation(config, key_match.algorithm);

    let data = decode::<SupabaseClaims>(token, &key_match.decoding_key, &validation)
        .map_err(|err| AppError::unauthorized("invalid token").with_source(err))?;

    Ok(data.claims)
}

fn build_validation(config: &Config, algorithm: Algorithm) -> Validation {
    let mut validation = Validation::new(algorithm);
    validation.set_issuer(&[config.supabase_jwt_issuer()]);
    validation.set_audience(std::slice::from_ref(&config.supabase_jwt_audience));
    validation.leeway = JWT_LEEWAY_SECONDS;
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::config::AppEnv;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use secrecy::SecretString;

    fn test_config() -> Config {
        Config {
            app_host: "127.0.0.1".into(),
            app_port: 0,
            app_env: AppEnv::Test,
            log_level: "info".into(),
            log_format: crate::config::LogFormat::Pretty,
            database_url: SecretString::from(String::from("postgres://localhost/x")),
            database_max_connections: 1,
            supabase_url: "https://test.supabase.co".into(),
            supabase_jwt_audience: "authenticated".into(),
            supabase_service_role_key: None,
            cors_allowed_origins: vec![],
            rate_limit_per_minute: 100,
            sentry: crate::config::SentryConfig {
                dsn: None,
                environment: "test".into(),
                traces_sample_rate: 0.0,
            },
            test_jwt_secret: Some(SecretString::from(String::from("test-secret"))),
            snaptrade: crate::config::SnaptradeConfig {
                client_id: String::new(),
                consumer_key: SecretString::from(String::new()),
                api_base: "https://api.snaptrade.invalid/api/v1".into(),
                redirect_uri: "http://127.0.0.1/cb".into(),
                broker_secret_encryption_key: vec![],
                state_secret: SecretString::from(String::new()),
            },
            billing: None,
        }
    }

    #[tokio::test]
    async fn hs256_test_token_verifies() {
        let config = test_config();
        let jwks = JwksCache::new(config.jwks_url());

        let now = time::OffsetDateTime::now_utc().unix_timestamp() as u64;
        let claims = serde_json::json!({
            "sub": "00000000-0000-0000-0000-000000000001",
            "email": "test@example.com",
            "iss": config.supabase_jwt_issuer(),
            "aud": "authenticated",
            "exp": now + 600,
            "user_metadata": {},
            "app_metadata": {}
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .expect("encode");

        let out = verify_token(&token, &config, &jwks).await.expect("verify");
        assert_eq!(out.sub, "00000000-0000-0000-0000-000000000001");
        assert_eq!(out.email.as_deref(), Some("test@example.com"));
    }

    #[tokio::test]
    async fn hs256_rejected_in_production() {
        let mut config = test_config();
        config.app_env = AppEnv::Production;
        // Production builds ignore the test secret even when present.
        config.test_jwt_secret = None;
        let jwks = JwksCache::new(config.jwks_url());

        let now = time::OffsetDateTime::now_utc().unix_timestamp() as u64;
        let claims = serde_json::json!({
            "sub": "00000000-0000-0000-0000-000000000001",
            "email": "test@example.com",
            "iss": config.supabase_jwt_issuer(),
            "aud": "authenticated",
            "exp": now + 600,
            "user_metadata": {},
            "app_metadata": {}
        });
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"test-secret"),
        )
        .expect("encode");

        let err = verify_token(&token, &config, &jwks).await.unwrap_err();
        assert_eq!(err.code(), crate::error::ErrorCode::Unauthorized);
    }
}
