//! Authentication: Supabase JWT verification and user upsert.

pub mod extractor;
pub mod jwks;
pub mod supabase_jwt;
pub mod upsert;

pub use extractor::AuthenticatedUser;
pub use jwks::JwksCache;
pub use supabase_jwt::{verify_token, SupabaseClaims};
