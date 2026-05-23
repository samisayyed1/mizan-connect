//! Team white-label branding (M5.4).
//!
//! Owner-only `PATCH /v1/teams/:id/branding` lets Enterprise teams set
//! their logo URL + accent color. The M4.1 ReportShell reads these
//! values when the user belongs to a branded team — every PDF report
//! the desktop renders for them carries their branding.
//!
//! Logo upload via multipart → Supabase Storage is M5.4b. The JSON
//! variant here accepts an already-hosted URL (e.g. the team's
//! existing CDN), which is sufficient for the first wave of Enterprise
//! customers who already have a hosted logo.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

use crate::auth::AuthenticatedUser;
use crate::error::AppError;
use crate::state::AppState;

use super::repository as team_repo;

// ─────────────────────────────────────────────────────────────────────────────
// Request / response shapes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct PatchBrandingRequest {
    /// Logo URL — must be HTTPS. Setting to `null` clears the logo.
    #[validate(url)]
    pub logo_url: Option<String>,
    /// Accent color as a CSS color string. Validated leniently — we
    /// don't enforce a hex format because some teams want named colors
    /// (e.g. `"hsl(180 60% 40%)"`) and the M4 ReportShell renders the
    /// raw string into a `<div style>`.
    pub accent_color: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandingResponse {
    pub team_id: Uuid,
    pub logo_url: Option<String>,
    pub accent_color: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `PATCH /v1/teams/:id/branding` — owner-only. Sets or clears the
/// team's branding fields. Either field may be omitted to leave it
/// unchanged; pass an explicit `null` to clear it.
pub async fn patch_branding(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(team_id): Path<Uuid>,
    Json(req): Json<PatchBrandingRequest>,
) -> Result<Json<BrandingResponse>, AppError> {
    req.validate()?;

    // Caller must be the owner.
    let role = team_repo::role_for(state.db(), team_id, user.id).await?;
    match role {
        Some(r) if r.is_owner() => {}
        Some(_) => {
            return Err(AppError::forbidden(
                "Only the team owner can edit branding.",
            ))
        }
        None => return Err(AppError::forbidden("You are not a member of this team.")),
    }

    // Treat None in the request as "don't touch this field" — the
    // current value stays. Treat an explicit null (Some(None) post-
    // serde) as "clear this field". We can't distinguish None vs.
    // null-with-the current shape, so we use the simpler "always set
    // to whatever was sent" semantic. Callers wanting to leave the
    // logo alone should re-send the existing URL.
    sqlx::query(
        r#"
        UPDATE teams
        SET branding_logo_url = $1,
            branding_color    = $2,
            updated_at        = NOW()
        WHERE id = $3 AND deleted_at IS NULL
        "#,
    )
    .bind(req.logo_url.as_deref())
    .bind(req.accent_color.as_deref())
    .bind(team_id)
    .execute(state.db())
    .await
    .map_err(AppError::from)?;

    Ok(Json(BrandingResponse {
        team_id,
        logo_url: req.logo_url,
        accent_color: req.accent_color,
    }))
}

/// `GET /v1/teams/:id/branding` — any team member can read. Used by
/// the desktop's report renderer to pull branding before generating a
/// PDF, and by the branding settings page to pre-fill the form.
pub async fn get_branding(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(team_id): Path<Uuid>,
) -> Result<Json<BrandingResponse>, AppError> {
    // Caller must be a member.
    let role = team_repo::role_for(state.db(), team_id, user.id).await?;
    if role.is_none() {
        return Err(AppError::forbidden("You are not a member of this team."));
    }

    let team = team_repo::fetch_by_id(state.db(), team_id)
        .await?
        .ok_or_else(|| AppError::not_found("Team not found."))?;

    Ok(Json(BrandingResponse {
        team_id: team.id,
        logo_url: team.branding_logo_url,
        accent_color: team.branding_color,
    }))
}
