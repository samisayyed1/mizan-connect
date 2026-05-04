//! Shared application state passed to every handler.

use std::sync::Arc;

use sqlx::PgPool;

use crate::auth::jwks::JwksCache;
use crate::config::Config;
use crate::snaptrade::client::SnaptradeClient;
use crate::snaptrade::encryption::EncryptionKey;
use crate::snaptrade::rate_limit::LoginPortalLimiter;

/// Application state cloned into every handler.
///
/// Cheap to clone — all heavy resources are behind `Arc`.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    config: Config,
    db: PgPool,
    jwks: JwksCache,
    snaptrade: SnaptradeClient,
    encryption: EncryptionKey,
    login_portal_limiter: LoginPortalLimiter,
}

impl AppState {
    pub fn new(
        config: Config,
        db: PgPool,
        jwks: JwksCache,
        snaptrade: SnaptradeClient,
        encryption: EncryptionKey,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                db,
                jwks,
                snaptrade,
                encryption,
                login_portal_limiter: LoginPortalLimiter::new(),
            }),
        }
    }

    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    pub fn db(&self) -> &PgPool {
        &self.inner.db
    }

    pub fn jwks(&self) -> &JwksCache {
        &self.inner.jwks
    }

    pub fn snaptrade(&self) -> &SnaptradeClient {
        &self.inner.snaptrade
    }

    pub fn encryption(&self) -> &EncryptionKey {
        &self.inner.encryption
    }

    pub fn login_portal_limiter(&self) -> &LoginPortalLimiter {
        &self.inner.login_portal_limiter
    }
}
