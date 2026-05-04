//! Request and response DTOs for the SnapTrade integration.
//!
//! Two layers:
//! 1. **SnapTrade DTOs** (private to this module) — match the upstream
//!    API verbatim. Used internally by [`crate::snaptrade::client`].
//! 2. **Public DTOs** (returned to the desktop) — the desktop's existing
//!    `crates/connect::types` shapes. We keep field names compatible to
//!    avoid touching the desktop client.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// SnapTrade upstream DTOs (private to the client)
// ---------------------------------------------------------------------------

/// Body for `POST /snapTrade/registerUser`.
#[derive(Debug, Serialize)]
pub(crate) struct StRegisterUserRequest<'a> {
    #[serde(rename = "userId")]
    pub user_id: &'a str,
}

/// Response from `POST /snapTrade/registerUser`.
#[derive(Debug, Deserialize)]
pub(crate) struct StRegisterUserResponse {
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "userSecret")]
    pub user_secret: String,
}

/// Body for `POST /snapTrade/login`.
#[derive(Debug, Serialize)]
pub(crate) struct StLoginRequest<'a> {
    #[serde(rename = "broker", skip_serializing_if = "Option::is_none")]
    pub broker: Option<&'a str>,
    #[serde(rename = "customRedirect")]
    pub custom_redirect: &'a str,
    #[serde(rename = "connectionType")]
    pub connection_type: &'a str,
    #[serde(rename = "connectionPortalVersion")]
    pub connection_portal_version: &'a str,
    #[serde(rename = "immediateRedirect", skip_serializing_if = "Option::is_none")]
    pub immediate_redirect: Option<bool>,
}

/// Response from `POST /snapTrade/login`.
#[derive(Debug, Deserialize)]
pub(crate) struct StLoginResponse {
    #[serde(rename = "redirectURI")]
    pub redirect_uri: String,
    #[serde(rename = "sessionId", default)]
    pub _session_id: Option<String>,
}

/// One element of `GET /authorizations`.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct StAuthorization {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub auth_type: Option<String>,
    pub disabled: bool,
    #[serde(default)]
    pub created_date: Option<String>,
    #[serde(default)]
    pub updated_date: Option<String>,
    pub brokerage: Option<StBrokerage>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct StBrokerage {
    pub id: String,
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub aws_s3_logo_url: Option<String>,
    #[serde(default)]
    pub aws_s3_square_logo_url: Option<String>,
    /// Preserved on deserialize to match SnapTrade's response surface.
    /// Not surfaced in our public DTO yet.
    #[serde(default, rename = "enabled")]
    pub _enabled: Option<bool>,
}

/// One element of `GET /accounts`.
#[derive(Debug, Deserialize, Clone)]
pub(crate) struct StAccount {
    pub id: String,
    #[serde(default)]
    pub brokerage_authorization: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub number: Option<String>,
    #[serde(default)]
    pub institution_name: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub raw_type: Option<String>,
    #[serde(default)]
    pub balance: Option<serde_json::Value>,
    #[serde(default)]
    pub sync_status: Option<serde_json::Value>,
    #[serde(default)]
    pub is_paper: Option<bool>,
}

// ---------------------------------------------------------------------------
// Public DTOs (returned by Mizan Connect — match desktop client shapes)
// ---------------------------------------------------------------------------

/// Response of `POST /api/v1/sync/brokerage/login-portal`.
#[derive(Debug, Serialize)]
pub struct LoginPortalResponse {
    pub url: String,
    /// RFC 3339 expiry of the SnapTrade portal URL itself (5 minutes per
    /// SnapTrade's docs; we surface it so the client can pre-empt expiry).
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// Brokerage authorization shape returned to the desktop. Mirrors the
/// shape `crates/connect::types::BrokerConnection` in the desktop repo.
#[derive(Debug, Serialize)]
pub struct BrokerConnectionDto {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub auth_type: Option<String>,
    pub disabled: bool,
    pub status: &'static str,
    pub brokerage: Option<BrokerageDto>,
    #[serde(rename = "created_date")]
    pub created_date: Option<String>,
    #[serde(rename = "updated_date")]
    pub updated_date: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct BrokerageDto {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub display_name: Option<String>,
    pub aws_s3_logo_url: Option<String>,
    pub aws_s3_square_logo_url: Option<String>,
}

/// Account DTO returned to desktop. Field names track the desktop's
/// `BrokerAccount` interface in `apps/frontend/src/features/mizan-connect/types.ts`.
#[derive(Debug, Serialize)]
pub struct BrokerAccountDto {
    pub id: String,
    pub brokerage_authorization: Option<String>,
    pub name: Option<String>,
    pub number: Option<String>,
    pub institution_name: Option<String>,
    pub status: Option<String>,
    pub raw_type: Option<String>,
    pub balance: Option<serde_json::Value>,
    pub sync_status: Option<serde_json::Value>,
    pub is_paper: bool,
}

/// Holdings response — opaque pass-through of SnapTrade's account
/// holdings payload. We do not unwrap shapes here because the desktop
/// already knows how to render the SnapTrade `AccountHoldings` schema.
pub type HoldingsDto = serde_json::Value;

/// Activities response — likewise an opaque pass-through.
pub type ActivitiesDto = serde_json::Value;

/// Pagination options for `/accounts/:id/activities`.
#[derive(Debug, Deserialize, Default)]
pub struct ActivitiesQuery {
    /// Cursor-based pagination per project convention. Internally
    /// translated to SnapTrade's `offset`. `None` = start at 0.
    #[serde(default)]
    pub cursor: Option<String>,
    /// Page size, defaults to 25 per CLAUDE.md, max 100.
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default, rename = "startDate")]
    pub start_date: Option<String>,
    #[serde(default, rename = "endDate")]
    pub end_date: Option<String>,
}

/// Internal — what we hand to the row layer.
#[derive(Debug)]
pub struct StoredConnection {
    pub id: Uuid,
    pub user_id: Uuid,
    pub snaptrade_user_id: String,
    pub snaptrade_user_secret_encrypted: Vec<u8>,
    pub snaptrade_authorization_id: Option<String>,
    pub broker_slug: Option<String>,
    pub institution_name: Option<String>,
    pub is_active: bool,
    pub disabled: bool,
}
