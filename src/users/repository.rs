//! User-related SQL.

use sqlx::PgPool;
use uuid::Uuid;

use super::model::User;

/// Fetch a user by primary key, ignoring soft-deleted rows.
pub async fn fetch_by_id(pool: &PgPool, id: Uuid) -> Result<Option<User>, sqlx::Error> {
    sqlx::query_as!(
        User,
        r#"
        SELECT id, supabase_user_id, email, display_name, avatar_url,
               created_at, updated_at, deleted_at
        FROM users
        WHERE id = $1 AND deleted_at IS NULL
        "#,
        id
    )
    .fetch_optional(pool)
    .await
}

/// Patch a user's display_name. Pass `None` to leave it unchanged.
pub async fn update_display_name(
    pool: &PgPool,
    id: Uuid,
    display_name: Option<&str>,
) -> Result<User, sqlx::Error> {
    sqlx::query_as!(
        User,
        r#"
        UPDATE users
           SET display_name = COALESCE($2, display_name)
         WHERE id = $1 AND deleted_at IS NULL
        RETURNING id, supabase_user_id, email, display_name, avatar_url,
                  created_at, updated_at, deleted_at
        "#,
        id,
        display_name,
    )
    .fetch_one(pool)
    .await
}
