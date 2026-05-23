//! Team invites (M5.3).
//!
//! Owners create invites at a role (`advisor` or `viewer`); the response
//! includes an opaque token that the desktop can render as an auth-
//! callback URL. The invitee opens that URL in the desktop, signs in
//! through Supabase, and the redeem endpoint inserts the team_members
//! row.
//!
//! Email delivery is best-effort: if `RESEND_API_KEY` is set the cloud
//! calls Resend; otherwise the URL is logged and shown in the response
//! so the inviter can DM it manually.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;
use validator::Validate;

use crate::auth::AuthenticatedUser;
use crate::error::AppError;
use crate::state::AppState;

use super::repository as team_repo;

/// Default invite TTL (7 days). Long enough that the user can sign up
/// over the weekend, short enough that stale tokens don't pile up.
const INVITE_TTL_DAYS: i64 = 7;

// ─────────────────────────────────────────────────────────────────────────────
// Request / response shapes
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Validate)]
#[serde(rename_all = "camelCase")]
pub struct CreateInviteRequest {
    #[validate(email)]
    pub email: String,
    /// Must be `advisor` or `viewer`. Owners can't be invited — there's
    /// at most one owner per team (the user who pays).
    pub role: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteResponse {
    pub token: String,
    pub team_id: Uuid,
    pub email: String,
    pub role: String,
    pub expires_at: String,
    /// The full URL to send to the invitee. Build with the desktop
    /// scheme so opening it lands on the auth-callback page with the
    /// `invite` query param set.
    pub redeem_url: String,
    /// True when the cloud delivered the email via Resend; false when
    /// it only logged the URL (dev mode / no RESEND_API_KEY).
    pub emailed: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteInfoResponse {
    pub team_id: Uuid,
    pub team_name: String,
    pub role: String,
    pub email: String,
    pub expires_at: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcceptInviteResponse {
    pub team_id: Uuid,
    pub team_name: String,
    pub role: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `POST /v1/teams/:id/invites` — owner-only. Generates a token, stores
/// the invite, and (if configured) emails it via Resend.
pub async fn create_invite(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(team_id): Path<Uuid>,
    Json(req): Json<CreateInviteRequest>,
) -> Result<Json<InviteResponse>, AppError> {
    req.validate()?;

    // Caller must be the owner of the team.
    let role = team_repo::role_for(state.db(), team_id, user.id).await?;
    match role {
        Some(r) if r.is_owner() => {}
        Some(_) => {
            return Err(AppError::forbidden(
                "Only the team owner can issue invites.",
            ))
        }
        None => return Err(AppError::forbidden("You are not a member of this team.")),
    }

    // Whitelist roles.
    let role = req.role.as_str();
    if role != "advisor" && role != "viewer" {
        return Err(AppError::bad_request("role must be 'advisor' or 'viewer'."));
    }

    let token = generate_token();
    let email = req.email.trim().to_lowercase();
    let expires_at = OffsetDateTime::now_utc() + Duration::days(INVITE_TTL_DAYS);

    sqlx::query(
        r#"
        INSERT INTO team_invites (token, team_id, invited_by, email, role, created_at, expires_at)
        VALUES ($1, $2, $3, $4, $5, NOW(), $6)
        "#,
    )
    .bind(&token)
    .bind(team_id)
    .bind(user.id)
    .bind(&email)
    .bind(role)
    .bind(expires_at)
    .execute(state.db())
    .await
    .map_err(AppError::from)?;

    // Build redeem URL. Uses the public app URL the cloud knows about;
    // the desktop opens it via the OS's URL-scheme handler, lands on
    // /auth/callback?invite=<token>, signs in, and accepts.
    let redeem_url = format!(
        "{}/auth/callback?invite={}",
        std::env::var("APP_PUBLIC_URL").unwrap_or_else(|_| "https://app.mizan.app".to_string()),
        token
    );

    // Best-effort email send. The function logs and returns whether it
    // actually delivered.
    let emailed = try_send_invite_email(&email, &redeem_url).await;

    Ok(Json(InviteResponse {
        token,
        team_id,
        email,
        role: role.to_string(),
        expires_at: expires_at.format(&Rfc3339).unwrap_or_default(),
        redeem_url,
        emailed,
    }))
}

/// `GET /v1/invites/:token` — public (no auth).
///
/// Returns the team name + role for the invitee so the desktop can
/// render "You've been invited to <Team Name> as <role>" before they
/// sign in.
pub async fn get_invite(
    State(state): State<AppState>,
    Path(token): Path<String>,
) -> Result<Json<InviteInfoResponse>, AppError> {
    let row: Option<(Uuid, String, String, String, OffsetDateTime, OffsetDateTime)> =
        sqlx::query_as(
            r#"
            SELECT i.team_id, t.name, i.role, i.email, i.expires_at, NOW() as now
            FROM team_invites i
            JOIN teams t ON t.id = i.team_id
            WHERE i.token = $1 AND i.redeemed_at IS NULL
            "#,
        )
        .bind(&token)
        .fetch_optional(state.db())
        .await
        .map_err(AppError::from)?;

    let (team_id, team_name, role, email, expires_at, now) =
        row.ok_or_else(|| AppError::not_found("Invite not found or already redeemed."))?;

    if expires_at <= now {
        return Err(AppError::not_found("Invite expired."));
    }

    Ok(Json(InviteInfoResponse {
        team_id,
        team_name,
        role,
        email,
        expires_at: expires_at.format(&Rfc3339).unwrap_or_default(),
    }))
}

/// `POST /v1/invites/:token/accept` — authenticated. The invitee's
/// email must match the invite's email (case-insensitive); on success
/// inserts a `team_members` row and marks the invite redeemed.
pub async fn accept_invite(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(token): Path<String>,
) -> Result<Json<AcceptInviteResponse>, AppError> {
    // Pull the invite + the team name in one query.
    let row: Option<(Uuid, String, String, String, OffsetDateTime)> = sqlx::query_as(
        r#"
        SELECT i.team_id, t.name, i.role, i.email, i.expires_at
        FROM team_invites i
        JOIN teams t ON t.id = i.team_id
        WHERE i.token = $1 AND i.redeemed_at IS NULL
        "#,
    )
    .bind(&token)
    .fetch_optional(state.db())
    .await
    .map_err(AppError::from)?;

    let (team_id, team_name, role, email, expires_at) =
        row.ok_or_else(|| AppError::not_found("Invite not found or already redeemed."))?;

    if expires_at <= OffsetDateTime::now_utc() {
        return Err(AppError::not_found("Invite expired."));
    }

    // Email match — case-insensitive.
    if user.email.to_lowercase() != email {
        return Err(AppError::forbidden(
            "This invite was issued to a different email address.",
        ));
    }

    // Atomic: insert member + mark invite redeemed. Both in a single TX
    // so a crash midway doesn't leave the invite half-redeemed.
    let mut tx = state.db().begin().await.map_err(AppError::from)?;

    sqlx::query(
        r#"
        INSERT INTO team_members (team_id, user_id, role, joined_at)
        VALUES ($1, $2, $3, NOW())
        ON CONFLICT (team_id, user_id) DO UPDATE SET role = EXCLUDED.role
        "#,
    )
    .bind(team_id)
    .bind(user.id)
    .bind(&role)
    .execute(&mut *tx)
    .await
    .map_err(AppError::from)?;

    sqlx::query(
        r#"
        UPDATE team_invites
        SET redeemed_at = NOW(), redeemed_by = $1
        WHERE token = $2
        "#,
    )
    .bind(user.id)
    .bind(&token)
    .execute(&mut *tx)
    .await
    .map_err(AppError::from)?;

    tx.commit().await.map_err(AppError::from)?;

    Ok(Json(AcceptInviteResponse {
        team_id,
        team_name,
        role,
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Generate an opaque 64-char hex invite token. Two UUID v4s back-to-back
/// give us 244 bits of entropy — same security ceiling as 32 random bytes
/// without pulling in the `rand` crate (uuid is already a transitive dep).
fn generate_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Best-effort Resend send. Returns true on success.
async fn try_send_invite_email(email: &str, redeem_url: &str) -> bool {
    let Ok(api_key) = std::env::var("RESEND_API_KEY") else {
        tracing::info!(
            target: "invites",
            email = email,
            redeem_url = redeem_url,
            "RESEND_API_KEY not set — invite URL logged but not emailed."
        );
        return false;
    };

    let from = std::env::var("RESEND_FROM").unwrap_or_else(|_| "noreply@mizan.app".to_string());
    let body = serde_json::json!({
        "from": from,
        "to": email,
        "subject": "You've been invited to a Mizan team",
        "html": format!(
            "<p>You've been invited to join a Mizan team.</p>\
             <p><a href=\"{}\">Open in Mizan</a></p>\
             <p>This invite expires in {} days.</p>",
            redeem_url, INVITE_TTL_DAYS
        )
    });

    let client = reqwest::Client::new();
    match client
        .post("https://api.resend.com/emails")
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
    {
        Ok(r) if r.status().is_success() => {
            tracing::info!(target: "invites", email = email, "Invite email sent.");
            true
        }
        Ok(r) => {
            tracing::warn!(
                target: "invites",
                email = email,
                status = r.status().as_u16(),
                "Resend returned non-2xx — falling back to URL-only."
            );
            false
        }
        Err(e) => {
            tracing::warn!(
                target: "invites",
                email = email,
                error = %e,
                "Resend request failed — falling back to URL-only."
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_64_hex_chars() {
        let t = generate_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn two_tokens_are_distinct() {
        // Trivially true with 256 bits of entropy, but worth asserting
        // the RNG isn't seeded deterministically.
        assert_ne!(generate_token(), generate_token());
    }
}
