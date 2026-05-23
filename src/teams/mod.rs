//! Teams feature: domain model, repository queries, and (in M5.2) router.
//!
//! The M5.1 cut ships the schema + types + repository only — the
//! existing `/v1/me` handler keeps working because the migration's
//! backfill assigns `team.id = user.id` for every solo user, so
//! `user_id`-keyed reads still find the right team UUID.
//!
//! M5.2 wires the advisor dashboard and switches `/v1/me` to look up
//! the user's primary team via `team_members` instead of the implicit
//! 1:1.

pub mod handlers;
pub mod model;
pub mod repository;

use axum::routing::get;
use axum::Router;

use crate::state::AppState;

/// `/v1/me/teams` + `/v1/teams/:id/members` routes.
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me/teams", get(handlers::list_for_me))
        .route("/teams/:id/members", get(handlers::list_members))
}
