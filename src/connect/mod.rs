//! `/api/v1/...` Connect-API stubs that haven't migrated to feature
//! modules yet.
//!
//! After Chunk 3, the only remaining stub here is `/subscription/plans`,
//! which is reachable with or without a bearer token (the desktop client
//! calls it from two distinct functions). Stripe billing in Chunk 4 will
//! replace this stub with a real handler.
//!
//! The brokerage routes that used to live here moved to
//! [`crate::snaptrade::router`].

pub mod handlers;

use axum::routing::get;
use axum::Router;

use crate::state::AppState;

/// Router mounted at `/api/v1`.
pub fn router() -> Router<AppState> {
    Router::new().route("/subscription/plans", get(handlers::subscription_plans))
}
