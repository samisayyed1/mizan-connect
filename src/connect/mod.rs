//! `/api/v1/...` Connect-API stubs.
//!
//! Chunk-1 placeholders for routes the Mizan desktop client expects to
//! exist. Each handler returns `501 Not Implemented` with the standard
//! `AppError` envelope so the client can render "Coming Soon" UI without
//! console-error spam. Real implementations land in Chunks 2–4.
//!
//! Auth policy:
//! - Brokerage / accounts / sync endpoints: behind `AuthenticatedUser`,
//!   so unauthenticated callers see 401 (matching production behaviour).
//! - `subscription/plans`: returns 501 regardless of auth. The desktop
//!   client calls this path with and without a bearer token from two
//!   different functions (`get_subscription_plans` and
//!   `fetch_subscription_plans_public`), both at the same URL — so the
//!   stub must be reachable in either state. Once Stripe ships in a
//!   later chunk, the authed variant will return the user's plan and
//!   the unauthed variant will keep returning the public price catalog.

pub mod handlers;

use axum::routing::get;
use axum::Router;

use crate::state::AppState;

/// Router mounted at `/api/v1`.
pub fn router() -> Router<AppState> {
    Router::new()
        // Reachable with or without auth — see module docstring.
        .route("/subscription/plans", get(handlers::subscription_plans))
        // Authed surfaces — return 401 first when no bearer token.
        .route(
            "/sync/brokerage/connections",
            get(handlers::brokerage_connections),
        )
        .route(
            "/sync/brokerage/accounts",
            get(handlers::brokerage_accounts),
        )
        .route(
            "/sync/brokerage/accounts/:id/activities",
            get(handlers::brokerage_account_activities),
        )
        .route(
            "/sync/brokerage/accounts/:id/holdings",
            get(handlers::brokerage_account_holdings),
        )
}
