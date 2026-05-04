//! JWKS (JSON Web Key Set) cache for Supabase signing keys.
//!
//! On startup the cache attempts an eager fetch. If unreachable, the failure
//! is logged at warn level and a lazy fetch happens on the first incoming
//! token. A background task refreshes the cache every hour. Each `get_keys`
//! call returns the most recent successful snapshot.

use std::sync::Arc;
use std::time::Duration;

use jsonwebtoken::DecodingKey;
use parking_lot::RwLock;
use serde::Deserialize;
use time::OffsetDateTime;

use crate::error::AppError;

const REFRESH_INTERVAL: Duration = Duration::from_secs(60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
const STALE_AFTER: Duration = Duration::from_secs(60 * 60 * 24);

/// In-memory JWKS cache. Cheap to clone — wraps `Arc`.
#[derive(Clone)]
pub struct JwksCache {
    inner: Arc<Inner>,
}

struct Inner {
    jwks_url: String,
    http: reqwest::Client,
    state: RwLock<CacheState>,
}

#[derive(Default)]
struct CacheState {
    keys: Vec<JwkEntry>,
    /// HS256 fallback for legacy Supabase projects, parsed from a base64
    /// secret returned by `/auth/v1/.well-known/jwks.json` on some plans.
    /// Currently unused — Supabase has migrated to RS256 JWKS for new
    /// projects.
    fetched_at: Option<OffsetDateTime>,
    last_error: Option<String>,
}

/// One usable signing key.
pub struct JwkEntry {
    pub kid: Option<String>,
    pub algorithm: jsonwebtoken::Algorithm,
    pub decoding_key: DecodingKey,
}

impl JwksCache {
    pub fn new(jwks_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(FETCH_TIMEOUT)
            .user_agent(concat!("mizan-connect/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            inner: Arc::new(Inner {
                jwks_url: jwks_url.into(),
                http,
                state: RwLock::new(CacheState::default()),
            }),
        }
    }

    /// Eager initial refresh. Errors are logged but not fatal — the lazy
    /// path will retry on the first request.
    pub async fn warm(&self) {
        if let Err(err) = self.refresh().await {
            tracing::warn!(error = %err, jwks_url = %self.inner.jwks_url,
                "JWKS warm-up failed; will lazy-fetch on first request");
        }
    }

    /// Spawn a background refresher that fires every hour.
    pub fn spawn_refresher(&self) -> tokio::task::JoinHandle<()> {
        let cache = self.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(REFRESH_INTERVAL);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker.tick().await; // skip the immediate first tick
            loop {
                ticker.tick().await;
                if let Err(err) = cache.refresh().await {
                    tracing::warn!(error = %err, "JWKS refresh failed");
                }
            }
        })
    }

    /// Fetch + parse the JWKS document and replace the in-memory cache.
    pub async fn refresh(&self) -> Result<(), JwksError> {
        let url = &self.inner.jwks_url;
        let document: JwksDocument = match async {
            self.inner
                .http
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .json::<JwksDocument>()
                .await
        }
        .await
        {
            Ok(doc) => doc,
            Err(err) => {
                self.inner.state.write().last_error = Some(err.to_string());
                return Err(JwksError::Http(err));
            }
        };

        let mut keys = Vec::with_capacity(document.keys.len());
        for jwk in document.keys.into_iter() {
            match jwk_to_entry(&jwk) {
                Ok(entry) => keys.push(entry),
                Err(err) => {
                    tracing::warn!(kid = ?jwk.kid, error = %err, "skipping JWKS entry");
                }
            }
        }

        if keys.is_empty() {
            self.inner.state.write().last_error = Some("empty JWKS document".to_string());
            return Err(JwksError::Empty);
        }

        let mut state = self.inner.state.write();
        state.keys = keys;
        state.fetched_at = Some(OffsetDateTime::now_utc());
        state.last_error = None;
        let count = state.keys.len();
        drop(state);

        tracing::info!(count, "JWKS refreshed");
        Ok(())
    }

    /// Locate a decoding key by `kid`. If `kid` is absent on the token, the
    /// first cached key is returned. If the cache is empty, a lazy refresh
    /// is attempted.
    pub async fn find_key(&self, kid: Option<&str>) -> Result<DecodingKeyMatch, AppError> {
        if self.is_empty() {
            // Lazy fetch.
            if let Err(err) = self.refresh().await {
                tracing::warn!(error = %err, "lazy JWKS fetch failed");
            }
        }

        let state = self.inner.state.read();
        let entry = match kid {
            Some(want) => state.keys.iter().find(|k| k.kid.as_deref() == Some(want)),
            None => state.keys.first(),
        };

        match entry {
            Some(e) => Ok(DecodingKeyMatch {
                algorithm: e.algorithm,
                decoding_key: e.decoding_key.clone(),
            }),
            None => Err(AppError::unauthorized("unknown signing key")),
        }
    }

    /// Whether the cache currently has any usable keys.
    pub fn is_empty(&self) -> bool {
        self.inner.state.read().keys.is_empty()
    }

    /// Health snapshot for `/ready`.
    pub fn snapshot(&self) -> JwksSnapshot {
        let state = self.inner.state.read();
        let stale = match state.fetched_at {
            Some(t) => OffsetDateTime::now_utc() - t > STALE_AFTER,
            None => true,
        };
        JwksSnapshot {
            key_count: state.keys.len(),
            fetched_at: state.fetched_at,
            stale,
            last_error: state.last_error.clone(),
        }
    }
}

/// Decoding key + algorithm to use for a single token.
pub struct DecodingKeyMatch {
    pub algorithm: jsonwebtoken::Algorithm,
    pub decoding_key: DecodingKey,
}

/// Read-only health snapshot of the JWKS cache.
pub struct JwksSnapshot {
    pub key_count: usize,
    pub fetched_at: Option<OffsetDateTime>,
    pub stale: bool,
    pub last_error: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum JwksError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JWKS document contained zero usable keys")]
    Empty,
}

#[derive(Debug, Deserialize)]
struct JwksDocument {
    keys: Vec<RawJwk>,
}

#[derive(Debug, Deserialize)]
struct RawJwk {
    kty: String,
    #[serde(default)]
    alg: Option<String>,
    #[serde(default)]
    kid: Option<String>,
    // RSA
    #[serde(default)]
    n: Option<String>,
    #[serde(default)]
    e: Option<String>,
    // EC
    #[serde(default)]
    crv: Option<String>,
    #[serde(default)]
    x: Option<String>,
    #[serde(default)]
    y: Option<String>,
    // oct (HS*)
    #[serde(default)]
    k: Option<String>,
}

fn jwk_to_entry(jwk: &RawJwk) -> Result<JwkEntry, String> {
    let alg = jwk.alg.as_deref().unwrap_or(default_alg_for_kty(&jwk.kty));
    let algorithm = parse_algorithm(alg)?;
    let decoding_key = match jwk.kty.as_str() {
        "RSA" => {
            let (n, e) = (
                jwk.n.as_deref().ok_or("RSA jwk missing n")?,
                jwk.e.as_deref().ok_or("RSA jwk missing e")?,
            );
            DecodingKey::from_rsa_components(n, e).map_err(|e| format!("invalid RSA jwk: {e}"))?
        }
        "EC" => {
            let (x, y) = (
                jwk.x.as_deref().ok_or("EC jwk missing x")?,
                jwk.y.as_deref().ok_or("EC jwk missing y")?,
            );
            let _ = jwk.crv.as_deref().ok_or("EC jwk missing crv")?;
            DecodingKey::from_ec_components(x, y).map_err(|e| format!("invalid EC jwk: {e}"))?
        }
        "oct" => {
            let k = jwk.k.as_deref().ok_or("oct jwk missing k")?;
            // Supabase legacy HS256 keys are sometimes published base64url.
            let raw = base64_url_decode(k)?;
            DecodingKey::from_secret(&raw)
        }
        other => return Err(format!("unsupported kty: {other}")),
    };

    Ok(JwkEntry {
        kid: jwk.kid.clone(),
        algorithm,
        decoding_key,
    })
}

fn default_alg_for_kty(kty: &str) -> &'static str {
    match kty {
        "RSA" => "RS256",
        "EC" => "ES256",
        "oct" => "HS256",
        _ => "RS256",
    }
}

fn parse_algorithm(alg: &str) -> Result<jsonwebtoken::Algorithm, String> {
    use jsonwebtoken::Algorithm::*;
    Ok(match alg {
        "HS256" => HS256,
        "HS384" => HS384,
        "HS512" => HS512,
        "RS256" => RS256,
        "RS384" => RS384,
        "RS512" => RS512,
        "ES256" => ES256,
        "ES384" => ES384,
        "PS256" => PS256,
        "PS384" => PS384,
        "PS512" => PS512,
        "EdDSA" => EdDSA,
        other => return Err(format!("unsupported alg: {other}")),
    })
}

fn base64_url_decode(input: &str) -> Result<Vec<u8>, String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD
        .decode(input)
        .map_err(|e| format!("base64 decode: {e}"))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn algorithm_parser_handles_common() {
        assert_eq!(
            parse_algorithm("RS256").unwrap(),
            jsonwebtoken::Algorithm::RS256
        );
        assert_eq!(
            parse_algorithm("HS256").unwrap(),
            jsonwebtoken::Algorithm::HS256
        );
        assert!(parse_algorithm("nope").is_err());
    }
}
