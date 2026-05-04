//! Short-lived HS256 state JWT for the SnapTrade callback flow.
//!
//! Issued in the `/login-portal` handler and embedded in the SnapTrade
//! `customRedirect` URL as a `state=<jwt>` query parameter. Verified on
//! the public callback to bind a redirect to the user who initiated it.
//!
//! Signing key: `MIZAN_SNAPTRADE_STATE_SECRET` (≥ 32 bytes after base64
//! decode, distinct from `MIZAN_TEST_JWT_SECRET`).
//!
//! Issuer is fixed at `mizan-snaptrade-state` so the state JWT can never
//! be confused with a Supabase session token, even if both happen to
//! share an HMAC key by accident.

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::error::AppError;

const ISSUER: &str = "mizan-snaptrade-state";
const TTL: Duration = Duration::minutes(10);
const LEEWAY_SECS: u64 = 30;

/// Claims embedded in the callback state JWT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateClaims {
    /// Local `users.id` UUID — the user who initiated this connect flow.
    pub sub: Uuid,
    /// Random per-issuance nonce; not currently inspected, but pins the
    /// JWT to a single issuance for future replay-detection work.
    pub nonce: Uuid,
    /// `mizan-snaptrade-state` — distinguishes from Supabase tokens.
    pub iss: String,
    /// Unix-seconds expiry. Set to now + 10 minutes at issuance.
    pub exp: i64,
    /// Unix-seconds issued-at.
    pub iat: i64,
}

/// Issue a fresh callback state JWT for `user_id`.
pub fn issue(user_id: Uuid, secret: &SecretString) -> Result<String, AppError> {
    let now = OffsetDateTime::now_utc();
    let exp = now + TTL;
    let claims = StateClaims {
        sub: user_id,
        nonce: Uuid::new_v4(),
        iss: ISSUER.to_string(),
        iat: now.unix_timestamp(),
        exp: exp.unix_timestamp(),
    };
    let key = EncodingKey::from_secret(secret.expose_secret().as_bytes());
    encode(&Header::new(Algorithm::HS256), &claims, &key)
        .map_err(|err| AppError::internal("could not issue state token").with_source(err))
}

/// Verify a callback state JWT.
///
/// Returns `Ok(claims)` on success, or `AppError::bad_request(...)` on
/// any failure (missing claims, expired, wrong issuer, bad signature).
/// Errors deliberately use a generic "invalid state" message — the
/// caller should not give an attacker information about *which* check
/// failed.
pub fn verify(token: &str, secret: &SecretString) -> Result<StateClaims, AppError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.set_issuer(&[ISSUER]);
    validation.validate_exp = true;
    validation.leeway = LEEWAY_SECS;
    // No audience claim — this token is internal-only.
    validation.required_spec_claims = ["exp", "iss"].iter().map(|s| s.to_string()).collect();

    let key = DecodingKey::from_secret(secret.expose_secret().as_bytes());
    let data = decode::<StateClaims>(token, &key, &validation)
        .map_err(|err| AppError::bad_request("invalid state token").with_source(err))?;
    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::ErrorCode;

    fn secret() -> SecretString {
        SecretString::from(String::from("a-test-state-secret-of-sufficient-length-32"))
    }

    #[test]
    fn round_trip_returns_same_user_id() {
        let user_id = Uuid::new_v4();
        let token = issue(user_id, &secret()).expect("issue");
        let claims = verify(&token, &secret()).expect("verify");
        assert_eq!(claims.sub, user_id);
        assert_eq!(claims.iss, ISSUER);
    }

    #[test]
    fn wrong_secret_rejects() {
        let user_id = Uuid::new_v4();
        let token = issue(user_id, &secret()).expect("issue");
        let other = SecretString::from(String::from(
            "a-different-secret-of-sufficient-32-bytes-please",
        ));
        let err = verify(&token, &other).unwrap_err();
        assert_eq!(err.code(), ErrorCode::BadRequest);
    }

    #[test]
    fn expired_token_rejects() {
        // Hand-craft an expired claim, sign with the real secret.
        let claims = StateClaims {
            sub: Uuid::new_v4(),
            nonce: Uuid::new_v4(),
            iss: ISSUER.to_string(),
            iat: OffsetDateTime::now_utc().unix_timestamp() - 7200,
            exp: OffsetDateTime::now_utc().unix_timestamp() - 3600,
        };
        let key = EncodingKey::from_secret(secret().expose_secret().as_bytes());
        let token = encode(&Header::new(Algorithm::HS256), &claims, &key).expect("encode");
        let err = verify(&token, &secret()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::BadRequest);
    }

    #[test]
    fn wrong_issuer_rejects() {
        let claims = StateClaims {
            sub: Uuid::new_v4(),
            nonce: Uuid::new_v4(),
            iss: "supabase".to_string(),
            iat: OffsetDateTime::now_utc().unix_timestamp(),
            exp: OffsetDateTime::now_utc().unix_timestamp() + 600,
        };
        let key = EncodingKey::from_secret(secret().expose_secret().as_bytes());
        let token = encode(&Header::new(Algorithm::HS256), &claims, &key).expect("encode");
        let err = verify(&token, &secret()).unwrap_err();
        assert_eq!(err.code(), ErrorCode::BadRequest);
    }
}
