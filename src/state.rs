//! Shared application state passed to every handler.

use std::sync::Arc;

use sqlx::PgPool;

use crate::auth::jwks::JwksCache;
use crate::billing::BillingContext;
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
    /// Optional — present only when Stripe + billing env vars are configured.
    /// Handlers fall back to `not_implemented` when this is `None`.
    billing: Option<BillingContext>,
}

impl AppState {
    pub fn new(
        config: Config,
        db: PgPool,
        jwks: JwksCache,
        snaptrade: SnaptradeClient,
        encryption: EncryptionKey,
        billing: Option<BillingContext>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                db,
                jwks,
                snaptrade,
                encryption,
                login_portal_limiter: LoginPortalLimiter::new(),
                billing,
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

    /// Billing context, when Stripe is configured. `None` collapses every
    /// billing endpoint to a clean `not_implemented` response.
    pub fn billing(&self) -> Option<&BillingContext> {
        self.inner.billing.as_ref()
    }
}
