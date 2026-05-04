//! SnapTrade integration (Chunk 3).
//!
//! Wraps the SnapTrade REST API and exposes the desktop-facing
//! `/api/v1/sync/brokerage/*` endpoints. The OAuth-style callback that
//! finalises the broker connection lives at
//! `/api/v1/sync/snaptrade/callback` and is the only public route in this
//! module — see [`handlers::snaptrade_callback`] and
//! [`state_token`] for the security model.
//!
//! Module map:
//! - [`signing`]: HMAC-SHA256 canonical-request signer (cites the Ruby SDK).
//! - [`encryption`]: AES-256-GCM at-rest encryption for SnapTrade `userSecret`.
//! - [`state_token`]: short-lived HS256 JWT bound to the local user.
//! - [`client`]: typed reqwest wrapper that signs every outbound request.
//! - [`rate_limit`]: per-user 10/hour bucket for `/login-portal`.
//! - [`repository`]: `broker_connections` SQLx queries.
//! - [`handlers`]: Axum handlers for the eight endpoints.

pub mod client;
pub mod encryption;
pub mod errors;
pub mod handlers;
pub mod rate_limit;
pub mod repository;
pub mod signing;
pub mod state_token;
pub mod types;

use axum::routing::{delete, get, post};
use axum::Router;

use crate::state::AppState;

/// Router mounted at `/api/v1`.
pub fn router() -> Router<AppState> {
    Router::new()
        // The callback is public (auth comes from the state JWT). Routed
        // first so it doesn't clash with anything.
        .route(
            "/sync/snaptrade/callback",
            get(handlers::snaptrade_callback),
        )
        // Auth-required endpoints.
        .route("/sync/brokerage/login-portal", post(handlers::login_portal))
        .route(
            "/sync/brokerage/connections",
            get(handlers::list_connections),
        )
        .route(
            "/sync/brokerage/connections/:id/refresh",
            post(handlers::refresh_connection),
        )
        .route(
            "/sync/brokerage/connections/:id",
            delete(handlers::delete_connection),
        )
        .route("/sync/brokerage/accounts", get(handlers::list_accounts))
        .route(
            "/sync/brokerage/accounts/:account_id/holdings",
            get(handlers::account_holdings),
        )
        .route(
            "/sync/brokerage/accounts/:account_id/activities",
            get(handlers::account_activities),
        )
}
