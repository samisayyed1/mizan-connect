//! `/v1/me` HTTP handlers.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;
use validator::Validate;

use crate::auth::AuthenticatedUser;
use crate::error::AppError;
use crate::state::AppState;
use crate::users::model::User;
use crate::users::repository;

/// Response shape for `GET /v1/me` and `PATCH /v1/me`.
#[derive(Debug, Serialize)]
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
}

impl From<User> for MeResponse {
    fn from(u: User) -> Self {
        Self {
            id: u.id,
            supabase_user_id: u.supabase_user_id,
            email: u.email,
            display_name: u.display_name,
            avatar_url: u.avatar_url,
            created_at: u.created_at,
            updated_at: u.updated_at,
        }
    }
}

/// Request body for `PATCH /v1/me`.
#[derive(Debug, Deserialize, Validate)]
pub struct PatchMeRequest {
    #[validate(length(min = 1, max = 100, message = "display_name must be 1-100 chars"))]
    pub display_name: Option<String>,
}

/// `GET /v1/me`
pub async fn get_me(user: AuthenticatedUser) -> Result<Json<MeResponse>, AppError> {
    Ok(Json(user.into_inner().into()))
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
    Ok(Json(updated.into()))
}
