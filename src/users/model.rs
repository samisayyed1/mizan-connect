//! User domain types.

use time::OffsetDateTime;
use uuid::Uuid;

/// Local mirror of a Supabase auth user. Keyed by `supabase_user_id`.
///
/// This is a domain type — never serialized over the wire directly. Convert
/// to a DTO (see `handlers::MeResponse`) before returning to clients.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub supabase_user_id: Uuid,
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub deleted_at: Option<OffsetDateTime>,
}
