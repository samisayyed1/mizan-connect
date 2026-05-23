//! `/v1/me` HTTP handlers.
//!
//! This is the load-bearing endpoint for the entire paid surface: the desktop
//! reads `team.entitlements` from here to decide what to gate. The shape
//! mirrors the desktop's `ApiUser` + `ApiTeam` deserializers in
//! `crates/connect/src/{client,broker/models}.rs`.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;
use uuid::Uuid;
use validator::Validate;

use crate::auth::AuthenticatedUser;
use crate::billing::entitlements::UNLIMITED;
use crate::billing::repository::SubscriptionRow;
use crate::billing::{entitlements_for, repository as billing_repo, AiCredits, Entitlements};
use crate::error::AppError;
use crate::state::AppState;
use crate::users::model::User;
use crate::users::repository;

// ─────────────────────────────────────────────────────────────────────────────
// Response shape
// ─────────────────────────────────────────────────────────────────────────────

/// Response shape for `GET /v1/me` and `PATCH /v1/me`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeResponse {
    pub id: Uuid,
    pub supabase_user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,

    pub team_id: Option<Uuid>,
    pub team_role: Option<String>,
    pub team: Option<TeamDto>,
}

/// Team envelope carrying subscription + entitlements. 1:1 with `User` until a
/// real teams table exists.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamDto {
    pub id: Uuid,
    pub name: String,
    pub logo_url: Option<String>,
    /// Plan slug — `free` / `basic` / `pro` / `enterprise` (or legacy slugs).
    pub plan: Option<String>,
    /// One of `active` / `trialing` / `past_due` / `canceled` / `incomplete` / etc.
    pub subscription_status: Option<String>,
    pub subscription_current_period_end: Option<String>,
    pub subscription_cancel_at_period_end: Option<bool>,
    pub canceled_at: Option<String>,
    pub country_code: Option<String>,
    pub created_at: Option<String>,
    /// Authoritative entitlements (Contract §A) — the desktop reads these
    /// directly instead of deriving from `plan` + `status`.
    pub entitlements: Entitlements,
    /// Managed-AI credit balance. `monthly == -1` ⇒ unlimited.
    pub ai_credits: AiCredits,
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// Request body for `PATCH /v1/me`.
#[derive(Debug, Deserialize, Validate)]
pub struct PatchMeRequest {
    #[validate(length(min = 1, max = 100, message = "display_name must be 1-100 chars"))]
    pub display_name: Option<String>,
}

/// `GET /v1/me`
pub async fn get_me(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Json<MeResponse>, AppError> {
    let sub = billing_repo::fetch_active(state.db(), user.id).await?;
    Ok(Json(build_me(user.into_inner(), sub)))
}

/// `PATCH /v1/me`
pub async fn patch_me(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(req): Json<PatchMeRequest>,
) -> Result<Json<MeResponse>, AppError> {
    req.validate()?;

    let display_name = req
        .display_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let updated = repository::update_display_name(state.db(), user.id, display_name).await?;
    let sub = billing_repo::fetch_active(state.db(), updated.id).await?;
    Ok(Json(build_me(updated, sub)))
}

// ─────────────────────────────────────────────────────────────────────────────
// Composition
// ─────────────────────────────────────────────────────────────────────────────

fn build_me(user: User, sub: Option<SubscriptionRow>) -> MeResponse {
    // Resolve entitlements + credits from the (optional) subscription row.
    // No row ⇒ Free; inactive status ⇒ Free per `entitlements_for`.
    let plan = sub.as_ref().map(|s| s.tier.clone());
    let status = sub.as_ref().map(|s| s.status.clone());
    let entitlements = entitlements_for(plan.as_deref(), status.as_deref());

    let resets_at = sub.as_ref().and_then(|s| s.current_period_end);
    let used = sub.as_ref().map(|s| s.ai_credits_used).unwrap_or(0);
    let ai_credits = AiCredits {
        monthly: entitlements.ai_credits_monthly,
        used: if entitlements.ai_credits_monthly == UNLIMITED {
            0
        } else {
            used
        },
        resets_at,
    };

    let team = TeamDto {
        id: user.id,
        name: user
            .display_name
            .clone()
            .unwrap_or_else(|| user.email.clone()),
        logo_url: None,
        plan,
        subscription_status: status,
        subscription_current_period_end: sub
            .as_ref()
            .and_then(|s| s.current_period_end)
            .and_then(|t| t.format(&Rfc3339).ok()),
        subscription_cancel_at_period_end: sub.as_ref().map(|s| s.cancel_at_period_end),
        canceled_at: None,
        country_code: None,
        created_at: user.created_at.format(&Rfc3339).ok(),
        entitlements,
        ai_credits,
    };

    MeResponse {
        id: user.id,
        supabase_user_id: user.supabase_user_id,
        email: user.email,
        display_name: user.display_name,
        avatar_url: user.avatar_url,
        created_at: user.created_at,
        updated_at: user.updated_at,
        team_id: Some(user.id),
        team_role: Some("owner".to_string()),
        team: Some(team),
    }
}
