//! User upsert from JWT claims.
//!
//! Called on every authenticated request after JWT verification. Uses
//! `ON CONFLICT (supabase_user_id)` to remain idempotent across concurrent
//! requests for the same Supabase user.

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::users::model::User;

use super::supabase_jwt::SupabaseClaims;

/// Upsert the local `users` row matching `claims.sub`. Returns the local row.
pub async fn upsert_user_from_claims(
    pool: &PgPool,
    claims: &SupabaseClaims,
) -> Result<User, AppError> {
    let supabase_user_id = Uuid::parse_str(&claims.sub)
        .map_err(|_| AppError::unauthorized("token sub is not a valid UUID"))?;

    let email = claims.email_required()?.to_string();
    let display_name = claims.display_name();
    let avatar_url = claims.avatar_url();

    let user = sqlx::query_as!(
        User,
        r#"
        INSERT INTO users (supabase_user_id, email, display_name, avatar_url)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (supabase_user_id) DO UPDATE
            SET email = EXCLUDED.email,
                display_name = COALESCE(EXCLUDED.display_name, users.display_name),
                avatar_url   = COALESCE(EXCLUDED.avatar_url,   users.avatar_url),
                updated_at   = NOW(),
                deleted_at   = NULL
        RETURNING
            id,
            supabase_user_id,
            email,
            display_name,
            avatar_url,
            created_at,
            updated_at,
            deleted_at
        "#,
        supabase_user_id,
        email,
        display_name,
        avatar_url,
    )
    .fetch_one(pool)
    .await?;

    Ok(user)
}
