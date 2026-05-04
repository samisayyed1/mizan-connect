//! Users feature: domain model, repository queries, and `/v1/me` handlers.

pub mod handlers;
pub mod model;
pub mod repository;

use axum::routing::{get, patch};
use axum::Router;

use crate::state::AppState;

/// `/v1/me` router.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me", get(handlers::get_me))
        .route("/me", patch(handlers::patch_me))
}
