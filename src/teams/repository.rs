//! Team repository queries (M5.1).
//!
//! Read-only in this cut — team creation, invites, and membership
//! mutations land in M5.2/M5.3. The handful of queries here let `/v1/me`
//! (and future advisor endpoints) resolve a user's team(s) and the
//! caller's role on a given team.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;

use super::model::{Team, TeamMember, TeamRole};

/// Fetch a team by id. Excludes soft-deleted rows.
pub async fn fetch_by_id(pool: &PgPool, team_id: Uuid) -> Result<Option<Team>, AppError> {
    let row = sqlx::query_as::<_, Team>(
        r#"
        SELECT id, name, owner_user_id, branding_logo_url, branding_color,
               created_at, updated_at, deleted_at
        FROM teams
        WHERE id = $1 AND deleted_at IS NULL
        "#,
    )
    .bind(team_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)?;
    Ok(row)
}

/// Every team the given user is a member of. Sorted by team name for
/// stable display order.
pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<Team>, AppError> {
    let rows = sqlx::query_as::<_, Team>(
        r#"
        SELECT t.id, t.name, t.owner_user_id, t.branding_logo_url, t.branding_color,
               t.created_at, t.updated_at, t.deleted_at
        FROM teams t
        JOIN team_members m ON m.team_id = t.id
        WHERE m.user_id = $1 AND t.deleted_at IS NULL
        ORDER BY LOWER(t.name) ASC
        "#,
    )
    .bind(user_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::from)?;
    Ok(rows)
}

/// The user's role on a given team, or None when they aren't a member.
pub async fn role_for(
    pool: &PgPool,
    team_id: Uuid,
    user_id: Uuid,
) -> Result<Option<TeamRole>, AppError> {
    let role_str: Option<(String,)> = sqlx::query_as(
        r#"
        SELECT role
        FROM team_members
        WHERE team_id = $1 AND user_id = $2
        "#,
    )
    .bind(team_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(AppError::from)?;

    // Treat unknown role strings as `viewer` so a future role insert that
    // hits an older binary degrades to read-only rather than 500.
    Ok(role_str.map(|(s,)| TeamRole::from_str(&s).unwrap_or(TeamRole::Viewer)))
}

/// All members of a given team. Used by the advisor dashboard (M5.2)
/// and the audit endpoint (M5.5).
#[allow(dead_code)] // wired in M5.2
pub async fn list_members(pool: &PgPool, team_id: Uuid) -> Result<Vec<TeamMember>, AppError> {
    let rows = sqlx::query_as::<_, TeamMember>(
        r#"
        SELECT team_id, user_id, role, joined_at
        FROM team_members
        WHERE team_id = $1
        ORDER BY joined_at ASC
        "#,
    )
    .bind(team_id)
    .fetch_all(pool)
    .await
    .map_err(AppError::from)?;
    Ok(rows)
}
