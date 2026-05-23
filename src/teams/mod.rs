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
pub mod invites;
pub mod model;
pub mod repository;

use axum::routing::{get, post};
use axum::Router;

use crate::state::AppState;

/// Routes:
/// - `GET  /me/teams`                — every team the caller is on (M5.2)
/// - `GET  /teams/:id/members`       — roster (M5.2)
/// - `POST /teams/:id/invites`       — owner-only invite creation (M5.3)
/// - `GET  /invites/:token`          — public invite info (M5.3)
/// - `POST /invites/:token/accept`   — authenticated invite redemption (M5.3)
pub fn router() -> Router<AppState> {
    Router::new()
        .route("/me/teams", get(handlers::list_for_me))
        .route("/teams/:id/members", get(handlers::list_members))
        .route("/teams/:id/invites", post(invites::create_invite))
        .route("/invites/:token", get(invites::get_invite))
        .route("/invites/:token/accept", post(invites::accept_invite))
}
