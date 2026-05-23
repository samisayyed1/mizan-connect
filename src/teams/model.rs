//! Team domain types (M5.1).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

/// One team — multi-user container that the subscription lives on.
///
/// In the M5.1 cut every user is the owner of a solo team backfilled by
/// the migration. The interesting cases (advisors with multiple clients,
/// branded reports) come online in M5.2 + M5.4.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Team {
    pub id: Uuid,
    pub name: String,
    pub owner_user_id: Uuid,
    pub branding_logo_url: Option<String>,
    pub branding_color: Option<String>,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
    pub deleted_at: Option<OffsetDateTime>,
}

/// One user's membership of a team.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TeamMember {
    pub team_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub joined_at: OffsetDateTime,
}

/// Valid `team_members.role` values. The check constraint enforces the
/// same set; this enum exists so callers don't have to stringify.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TeamRole {
    /// Full read/write on the team's portfolio + billing. Backfilled
    /// onto every existing user.
    Owner,
    /// Read/write on portfolio data, no billing. (M5.2 advisor mode.)
    Advisor,
    /// Read-only on portfolio data. No billing, no mutations.
    Viewer,
}

impl TeamRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Advisor => "advisor",
            Self::Viewer => "viewer",
        }
    }

    /// Parse a string from the DB into a role. Anything unexpected is
    /// surfaced as None so the caller can decide whether to reject or
    /// silently treat it as viewer.
    ///
    /// Named `from_str` for symmetry with `as_str`; we don't implement
    /// the `std::str::FromStr` trait because the trait variant errors
    /// rather than returning `Option`, and the optional shape is what
    /// every call site wants.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "owner" => Some(Self::Owner),
            "advisor" => Some(Self::Advisor),
            "viewer" => Some(Self::Viewer),
            _ => None,
        }
    }

    /// Owner-only check used by billing endpoints + branding upload.
    pub fn is_owner(&self) -> bool {
        matches!(self, Self::Owner)
    }

    /// Whether this role may mutate portfolio data (excludes billing).
    pub fn can_write_portfolio(&self) -> bool {
        matches!(self, Self::Owner | Self::Advisor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_round_trips_through_string() {
        for r in [TeamRole::Owner, TeamRole::Advisor, TeamRole::Viewer] {
            assert_eq!(TeamRole::from_str(r.as_str()), Some(r));
        }
    }

    #[test]
    fn unknown_role_returns_none() {
        assert_eq!(TeamRole::from_str("auditor"), None);
        assert_eq!(TeamRole::from_str(""), None);
    }

    #[test]
    fn owner_has_full_permissions() {
        assert!(TeamRole::Owner.is_owner());
        assert!(TeamRole::Owner.can_write_portfolio());
    }

    #[test]
    fn advisor_can_write_but_is_not_owner() {
        assert!(!TeamRole::Advisor.is_owner());
        assert!(TeamRole::Advisor.can_write_portfolio());
    }

    #[test]
    fn viewer_is_read_only() {
        assert!(!TeamRole::Viewer.is_owner());
        assert!(!TeamRole::Viewer.can_write_portfolio());
    }
}
