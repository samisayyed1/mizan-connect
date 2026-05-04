//! Shared application state passed to every handler.

use std::sync::Arc;

use sqlx::PgPool;

use crate::auth::jwks::JwksCache;
use crate::config::Config;

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
}

impl AppState {
    pub fn new(config: Config, db: PgPool, jwks: JwksCache) -> Self {
        Self {
            inner: Arc::new(Inner { config, db, jwks }),
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
}
