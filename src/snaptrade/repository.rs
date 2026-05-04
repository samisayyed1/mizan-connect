//! `broker_connections` SQL for the SnapTrade flow.
//!
//! All queries use SQLx macros so the schema is checked at compile time.
//! The encrypted `userSecret` blob (`snaptrade_user_secret_encrypted`) is
//! stored as `BYTEA`; callers in [`crate::snaptrade::handlers`] are
//! responsible for round-tripping it via
//! [`crate::snaptrade::encryption::EncryptionKey`].

use sqlx::PgPool;
use uuid::Uuid;

use crate::error::AppError;
use crate::snaptrade::types::StoredConnection;

/// Insert a new pending connection (post-`registerUser`, pre-callback).
/// `is_active` defaults to false; the callback flips it to true once the
/// user actually completes the SnapTrade portal flow.
pub async fn insert_pending(
    pool: &PgPool,
    user_id: Uuid,
    snaptrade_user_id: &str,
    snaptrade_user_secret_encrypted: &[u8],
) -> Result<Uuid, AppError> {
    let id = sqlx::query_scalar!(
        r#"
        INSERT INTO broker_connections (
            user_id,
            snaptrade_user_id,
            snaptrade_user_secret_encrypted,
            connection_type,
            is_active
        )
        VALUES ($1, $2, $3, 'snaptrade', FALSE)
        RETURNING id
        "#,
        user_id,
        snaptrade_user_id,
        snaptrade_user_secret_encrypted,
    )
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Look up the active SnapTrade connection for a user, if any. There can
/// be at most one (enforced by partial unique index).
pub async fn fetch_active_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<StoredConnection>, AppError> {
    let row = sqlx::query_as!(
        StoredConnection,
        r#"
        SELECT id,
               user_id,
               snaptrade_user_id,
               snaptrade_user_secret_encrypted,
               snaptrade_authorization_id,
               broker_slug,
               institution_name,
               is_active,
               disabled
        FROM broker_connections
        WHERE user_id = $1
          AND connection_type = 'snaptrade'
          AND is_active = TRUE
        LIMIT 1
        "#,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Look up any (active or pending) SnapTrade row for a user. Used by the
/// login-portal handler to reuse an existing `(userId, userSecret)` pair
/// across re-attempts.
pub async fn fetch_any_for_user(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<StoredConnection>, AppError> {
    let row = sqlx::query_as!(
        StoredConnection,
        r#"
        SELECT id,
               user_id,
               snaptrade_user_id,
               snaptrade_user_secret_encrypted,
               snaptrade_authorization_id,
               broker_slug,
               institution_name,
               is_active,
               disabled
        FROM broker_connections
        WHERE user_id = $1
          AND connection_type = 'snaptrade'
        ORDER BY created_at DESC
        LIMIT 1
        "#,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Look up by SnapTrade `snaptrade_user_id`. Used by the public callback
/// to bind a redirect to a user (after state-token verification has
/// already restricted to a known `users.id`).
pub async fn fetch_by_snaptrade_user(
    pool: &PgPool,
    snaptrade_user_id: &str,
) -> Result<Option<StoredConnection>, AppError> {
    let row = sqlx::query_as!(
        StoredConnection,
        r#"
        SELECT id,
               user_id,
               snaptrade_user_id,
               snaptrade_user_secret_encrypted,
               snaptrade_authorization_id,
               broker_slug,
               institution_name,
               is_active,
               disabled
        FROM broker_connections
        WHERE snaptrade_user_id = $1
          AND connection_type = 'snaptrade'
        LIMIT 1
        "#,
        snaptrade_user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// Mark a connection as completed. Idempotent — calling twice with the
/// same `authorization_id` is a no-op (rows match the WHERE).
pub async fn mark_completed(
    pool: &PgPool,
    connection_id: Uuid,
    snaptrade_authorization_id: &str,
    broker_slug: Option<&str>,
    institution_name: Option<&str>,
) -> Result<(), AppError> {
    sqlx::query!(
        r#"
        UPDATE broker_connections
           SET snaptrade_authorization_id = $2,
               broker_slug = COALESCE($3, broker_slug),
               institution_name = COALESCE($4, institution_name),
               is_active = TRUE,
               disabled = FALSE,
               updated_at = NOW()
         WHERE id = $1
        "#,
        connection_id,
        snaptrade_authorization_id,
        broker_slug,
        institution_name,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Soft-delete a connection: clears `is_active` and sets `disabled`. The
/// row sticks around so future audit queries can see it was once present.
/// Returns the number of rows affected (0 = already gone).
pub async fn soft_delete(
    pool: &PgPool,
    user_id: Uuid,
    connection_id: Uuid,
) -> Result<u64, AppError> {
    let result = sqlx::query!(
        r#"
        UPDATE broker_connections
           SET is_active = FALSE,
               disabled = TRUE,
               updated_at = NOW()
         WHERE id = $1 AND user_id = $2 AND connection_type = 'snaptrade'
        "#,
        connection_id,
        user_id,
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// Look up by id + user. Use for authorization checks before mutating
/// (e.g. DELETE / refresh).
pub async fn fetch_by_id_for_user(
    pool: &PgPool,
    user_id: Uuid,
    connection_id: Uuid,
) -> Result<Option<StoredConnection>, AppError> {
    let row = sqlx::query_as!(
        StoredConnection,
        r#"
        SELECT id,
               user_id,
               snaptrade_user_id,
               snaptrade_user_secret_encrypted,
               snaptrade_authorization_id,
               broker_slug,
               institution_name,
               is_active,
               disabled
        FROM broker_connections
        WHERE id = $1 AND user_id = $2 AND connection_type = 'snaptrade'
        "#,
        connection_id,
        user_id,
    )
    .fetch_optional(pool)
    .await?;
    Ok(row)
}
