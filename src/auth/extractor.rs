//! `AuthenticatedUser` Axum extractor.
//!
//! Reads the bearer token from `Authorization`, verifies it via JWKS, and
//! upserts the local `users` row before handing the row to the handler.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::{header, StatusCode};

use crate::error::AppError;
use crate::state::AppState;
use crate::users::model::User;

/// Authenticated end-user. Always backed by a real local `users` row.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser(pub User);

impl AuthenticatedUser {
    pub fn into_inner(self) -> User {
        self.0
    }
}

impl std::ops::Deref for AuthenticatedUser {
    type Target = User;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[axum::async_trait]
impl FromRequestParts<AppState> for AuthenticatedUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        let auth_value = parts
            .headers
            .get(header::AUTHORIZATION)
            .ok_or_else(|| AppError::unauthorized("missing Authorization header"))?
            .to_str()
            .map_err(|_| AppError::unauthorized("Authorization header is not valid UTF-8"))?;

        let token = parse_bearer(auth_value)?;
        let claims = crate::auth::verify_token(token, state.config(), state.jwks()).await?;
        let user = crate::auth::upsert::upsert_user_from_claims(state.db(), &claims).await?;

        // Light surface-level audit trail; deeper events go via dedicated
        // audit-log calls inside handlers.
        tracing::debug!(
            user_id = %user.id,
            supabase_user_id = %user.supabase_user_id,
            "authenticated request"
        );

        Ok(Self(user))
    }
}

fn parse_bearer(value: &str) -> Result<&str, AppError> {
    let value = value.trim();
    let bearer = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .ok_or_else(|| AppError::unauthorized("expected Bearer token"))?;
    let token = bearer.trim();
    if token.is_empty() {
        return Err(AppError::unauthorized("empty bearer token"));
    }
    Ok(token)
}

// Sanity check: the public extractor must produce 401 by default.
const _: fn() = || {
    assert!(StatusCode::UNAUTHORIZED.is_client_error());
};

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn parses_bearer_with_lower_and_upper_case() {
        assert_eq!(parse_bearer("Bearer abc.def.ghi").unwrap(), "abc.def.ghi");
        assert_eq!(parse_bearer("bearer abc").unwrap(), "abc");
        assert!(parse_bearer("abc").is_err());
        assert!(parse_bearer("Bearer ").is_err());
    }
}
