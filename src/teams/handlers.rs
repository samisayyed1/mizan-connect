//! Teams HTTP handlers (M5.2).
//!
//! Exposes a small read-only surface so the desktop's advisor dashboard
//! (and future white-label admin UI) can list the teams the caller has
//! access to, and resolve their role on each.

use axum::extract::{Path, State};
use axum::Json;
use serde::Serialize;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use crate::auth::AuthenticatedUser;
use crate::error::AppError;
use crate::state::AppState;
use crate::users::repository as user_repo;

use super::model::{Team, TeamMember};
use super::repository;

// ─────────────────────────────────────────────────────────────────────────────
// Response shapes
// ─────────────────────────────────────────────────────────────────────────────

/// One team in a list response. Mirrors `Team` minus internal-only fields.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamSummary {
    pub id: Uuid,
    pub name: String,
    pub owner_user_id: Uuid,
    pub branding_logo_url: Option<String>,
    pub branding_color: Option<String>,
    /// The caller's role on this team. None when the caller isn't a
    /// member (should never happen for `list_for_me`, but kept optional
    /// so the type composes cleanly).
    pub my_role: Option<String>,
    pub created_at: Option<String>,
}

impl TeamSummary {
    fn from_team(team: Team, my_role: Option<String>) -> Self {
        Self {
            id: team.id,
            name: team.name,
            owner_user_id: team.owner_user_id,
            branding_logo_url: team.branding_logo_url,
            branding_color: team.branding_color,
            my_role,
            created_at: team.created_at.format(&Rfc3339).ok(),
        }
    }
}

/// `GET /v1/me/teams` response envelope.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MyTeamsResponse {
    pub teams: Vec<TeamSummary>,
}

/// One team member in a list response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMemberDto {
    pub user_id: Uuid,
    pub role: String,
    pub joined_at: Option<String>,
    /// Display name copied from the user row (or email if blank).
    pub display_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMembersResponse {
    pub team_id: Uuid,
    pub members: Vec<TeamMemberDto>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `GET /v1/me/teams` — every team the caller is a member of, with the
/// caller's role on each. Used by the desktop advisor dashboard to
/// render the team-picker tile grid.
pub async fn list_for_me(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Json<MyTeamsResponse>, AppError> {
    let user_id = user.id;
    let teams = repository::list_for_user(state.db(), user_id).await?;

    // For each team, resolve the caller's role. Done in a loop rather
    // than a single SQL with a join because the team list is small
    // (typically 1–3, max maybe 20 for a busy advisor) and this keeps
    // the repository surface minimal.
    let mut summaries = Vec::with_capacity(teams.len());
    for team in teams {
        let role = repository::role_for(state.db(), team.id, user_id).await?;
        summaries.push(TeamSummary::from_team(
            team,
            role.map(|r| r.as_str().to_string()),
        ));
    }

    Ok(Json(MyTeamsResponse { teams: summaries }))
}

/// `GET /v1/teams/{id}/members` — list members of a team. Caller must
/// be a member; viewers can see the roster but not the audit log.
pub async fn list_members(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(team_id): Path<Uuid>,
) -> Result<Json<TeamMembersResponse>, AppError> {
    let user_id = user.id;

    // Caller must be on the team.
    let my_role = repository::role_for(state.db(), team_id, user_id).await?;
    if my_role.is_none() {
        return Err(AppError::forbidden("You are not a member of this team."));
    }

    let members = repository::list_members(state.db(), team_id).await?;

    // Join member rows with display names. Tiny N — load each user's
    // name with a per-row fetch; cleaner than threading a batched query
    // for a feature that will rarely have >50 members.
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        let TeamMember {
            user_id: member_user_id,
            role,
            joined_at,
            ..
        } = m;
        let display_name = user_repo::fetch_by_id(state.db(), member_user_id)
            .await
            .ok()
            .flatten()
            .and_then(|u| u.display_name.or(Some(u.email)));
        out.push(TeamMemberDto {
            user_id: member_user_id,
            role,
            joined_at: joined_at.format(&Rfc3339).ok(),
            display_name,
        });
    }

    Ok(Json(TeamMembersResponse {
        team_id,
        members: out,
    }))
}
