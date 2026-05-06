//! Axum handlers for `/api/v1/sync/brokerage/*` and the SnapTrade
//! callback at `/api/v1/sync/snaptrade/callback`.
//!
//! Authorization model:
//! - Every brokerage handler takes `AuthenticatedUser`.
//! - The callback is the lone public route in this module — it relies on
//!   the HS256 state JWT (issued by `/login-portal`) to bind a redirect
//!   to the user who initiated the connect flow.
//!
//! Audit logging follows the table in CLAUDE.md:
//!
//! | event_type                 | event_data                                     |
//! |----------------------------|------------------------------------------------|
//! | `broker.connect.initiated` | `{ user_id, snaptrade_user_id }`               |
//! | `broker.connect.completed` | `{ user_id, authorization_id, broker_slug }`   |
//! | `broker.connect.failed`    | `{ user_id, error_code, snaptrade_status }`    |
//! | `broker.disconnect`        | `{ user_id, authorization_id }`                |
//! | `broker.refresh`           | `{ user_id, authorization_id }`                |

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::audit;
use crate::auth::AuthenticatedUser;
use crate::error::{AppError, ErrorCode};
use crate::snaptrade::repository;
use crate::snaptrade::state_token;
use crate::snaptrade::types::{
    ActivitiesQuery, BrokerAccountDto, BrokerConnectionDto, BrokerageDto, LoginPortalResponse,
    StAccount, StAuthorization,
};
use crate::state::AppState;

const ACTIVITIES_DEFAULT_LIMIT: u32 = 25;
const ACTIVITIES_MAX_LIMIT: u32 = 100;
const PORTAL_TTL_MINUTES: i64 = 5;

// ---------------------------------------------------------------------------
// POST /api/v1/sync/brokerage/login-portal
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct LoginPortalRequest {
    /// Optional broker slug to deep-link directly into a broker. If omitted
    /// the user is shown SnapTrade's broker picker.
    #[serde(default)]
    pub broker: Option<String>,
}

/// `POST /api/v1/sync/brokerage/login-portal`
pub async fn login_portal(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(_req): Json<LoginPortalRequest>,
) -> Result<Json<LoginPortalResponse>, AppError> {
    // Per-user rate limit (10/hour) on top of the global IP-based limiter.
    if let Err(retry_after) = state.login_portal_limiter().check(user.id) {
        let secs = retry_after.whole_seconds().max(0);
        tracing::warn!(user_id = %user.id, retry_after_secs = secs, "login-portal rate limited");
        return Err(AppError::new(
            ErrorCode::TooManyRequests,
            "too many login-portal requests; try again later",
        ));
    }

    let cfg = state.config();
    let pool = state.db();
    let snaptrade = state.snaptrade();
    let encryption = state.encryption();

    // Reuse an existing SnapTrade (userId, userSecret) pair if one exists,
    // otherwise lazily register the user with SnapTrade.
    let stored = repository::fetch_any_for_user(pool, user.id).await?;
    let (snaptrade_user_id, user_secret_plain) = match stored.as_ref() {
        Some(row) => {
            let secret = encryption
                .decrypt(&row.snaptrade_user_secret_encrypted)
                .map_err(|e| {
                    tracing::error!(error = %e, "decrypt stored SnapTrade userSecret");
                    AppError::internal("broker integration error")
                })?;
            (row.snaptrade_user_id.clone(), secret)
        }
        None => {
            let st_user_id = user.id.to_string();
            let resp = snaptrade.register_user(&st_user_id).await?;
            // Audit BEFORE the row insert so a duplicate write at least
            // captures the registration attempt.
            let _ = audit::record_event(
                pool,
                audit::AuditEvent::new("broker.connect.initiated")
                    .user(user.id)
                    .data(&json!({ "snaptrade_user_id": resp.user_id })),
            )
            .await;
            let blob = encryption.encrypt(&resp.user_secret).map_err(|e| {
                tracing::error!(error = %e, "encrypt fresh SnapTrade userSecret");
                AppError::internal("broker integration error")
            })?;
            let _ = repository::insert_pending(pool, user.id, &resp.user_id, &blob).await?;
            (resp.user_id, secrecy::SecretString::from(resp.user_secret))
        }
    };

    // Issue the callback state JWT and embed it in customRedirect.
    let state_jwt = state_token::issue(user.id, &cfg.snaptrade.state_secret)?;
    let mut redirect_with_state = url::Url::parse(&cfg.snaptrade.redirect_uri).map_err(|e| {
        tracing::error!(error = %e, "invalid SNAPTRADE_REDIRECT_URI");
        AppError::internal("broker integration misconfigured")
    })?;
    redirect_with_state
        .query_pairs_mut()
        .append_pair("state", &state_jwt);

    let login = snaptrade
        .login_user(
            &snaptrade_user_id,
            &user_secret_plain,
            redirect_with_state.as_str(),
        )
        .await?;

    let expires_at = OffsetDateTime::now_utc() + Duration::minutes(PORTAL_TTL_MINUTES);
    Ok(Json(LoginPortalResponse {
        url: login.redirect_uri,
        expires_at,
    }))
}

// ---------------------------------------------------------------------------
// GET /api/v1/sync/snaptrade/callback  (PUBLIC — bound by state JWT)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct CallbackQuery {
    /// State JWT issued by `/login-portal` (passed through SnapTrade
    /// untouched in `customRedirect`). Made `Option` so we can reject
    /// missing-state with our own `AppError` envelope rather than
    /// Axum's default extraction-rejection response.
    #[serde(default)]
    pub state: Option<String>,
    /// SnapTrade user identifier as it appears in our `broker_connections`
    /// row (we set this to our local users.id UUID at registerUser time).
    /// SnapTrade's portal redirect carries `state` and `userId` only —
    /// notably NOT `authorizationId` — so we look up the authorization
    /// via `GET /authorizations` after verifying state.
    #[serde(default, rename = "userId", alias = "snapTradeUserId")]
    pub user_id: Option<String>,
}

/// `GET /api/v1/sync/snaptrade/callback`
///
/// Public endpoint. Verifies the state JWT, resolves the user, then calls
/// SnapTrade's `GET /authorizations` to find the authorization that was
/// just created (SnapTrade's portal redirect does NOT carry an
/// `authorizationId` query param — only `state` + `userId`). Persists the
/// resolved authorization id and returns an HTML page.
///
/// Idempotency: the table has at most one active SnapTrade row per user
/// (enforced by the partial unique index `uq_broker_conn_user_snaptrade`).
/// Replay calls find the same authorization already on the row, treat
/// the candidate set as empty, and return the success HTML without
/// touching the DB or audit log.
///
/// Returns 400 only for verification failures (missing/bad state, no
/// pending row, userId mismatch). Upstream `/authorizations` failures
/// surface as the standard SnapTrade `AppError`. The "no new
/// authorization found" path renders a user-friendly 200 HTML page —
/// SnapTrade redirected the user here, so a JSON 4xx would be jarring.
pub async fn snaptrade_callback(
    State(state): State<AppState>,
    Query(q): Query<CallbackQuery>,
) -> Result<Response, AppError> {
    let cfg = state.config();
    let pool = state.db();

    let state_jwt = q.state.as_deref().ok_or_else(|| {
        record_failure_locked(pool, None, "missing_state", None);
        AppError::bad_request("missing state token")
    })?;

    let claims = state_token::verify(state_jwt, &cfg.snaptrade.state_secret).inspect_err(|_| {
        record_failure_locked(pool, None, "bad_state", None);
    })?;
    let local_user_id = claims.sub;

    // Look up the connection by user; if SnapTrade also passed back userId
    // we cross-check it for defense-in-depth.
    let row = repository::fetch_any_for_user(pool, local_user_id)
        .await?
        .ok_or_else(|| {
            record_failure_locked(pool, Some(local_user_id), "no_pending_connection", None);
            AppError::bad_request("no pending connection for this user")
        })?;

    if let Some(returned) = q.user_id.as_deref() {
        if returned != row.snaptrade_user_id {
            record_failure_locked(pool, Some(local_user_id), "user_id_mismatch", None);
            return Err(AppError::bad_request("snaptrade userId mismatch"));
        }
    }

    // Fetch the user's authorizations from SnapTrade and pick the newly
    // created one. Any authorization already on the row is filtered out
    // so callback replays are no-ops.
    let user_secret = state
        .encryption()
        .decrypt(&row.snaptrade_user_secret_encrypted)?;
    let authorizations = state
        .snaptrade()
        .list_authorizations(&row.snaptrade_user_id, &user_secret)
        .await
        .inspect_err(|_| {
            record_failure_locked(
                pool,
                Some(local_user_id),
                "list_authorizations_failed",
                None,
            );
        })?;

    let existing_id = row.snaptrade_authorization_id.as_deref();
    let chosen = pick_newest_authorization(&authorizations, existing_id);

    let Some(auth) = chosen else {
        // No new authorization. If the row already has one, this is a
        // replay → success page (no DB write, no duplicate audit). If it
        // doesn't, SnapTrade redirected the user here without recording an
        // authorization → render the user-friendly "didn't complete" page.
        if existing_id.is_some() {
            return Ok((StatusCode::OK, Html(success_html())).into_response());
        }
        record_failure_locked(pool, Some(local_user_id), "no_new_authorization", None);
        return Ok((StatusCode::OK, Html(connection_failed_html())).into_response());
    };

    let broker_slug = auth.brokerage.as_ref().map(|b| b.slug.as_str());
    let institution_name = auth
        .brokerage
        .as_ref()
        .and_then(|b| b.display_name.as_deref().or(Some(b.name.as_str())));
    repository::mark_completed(pool, row.id, &auth.id, broker_slug, institution_name).await?;

    let _ = audit::record_event(
        pool,
        audit::AuditEvent::new("broker.connect.completed")
            .user(local_user_id)
            .data(&json!({
                "authorization_id": auth.id,
                "broker_slug": broker_slug,
            })),
    )
    .await;

    Ok((StatusCode::OK, Html(success_html())).into_response())
}

/// Pick the newest [`StAuthorization`] from `authorizations` that is not
/// already recorded on the row (`existing_id`).
///
/// Newness is determined by parsing `created_date` as RFC 3339; entries
/// with a missing or unparseable timestamp rank as oldest. Ties are
/// broken by taking the entry that appears later in the slice
/// (SnapTrade's response order). Returns `None` if every entry matches
/// `existing_id` (replay) or the slice is empty.
fn pick_newest_authorization<'a>(
    authorizations: &'a [StAuthorization],
    existing_id: Option<&str>,
) -> Option<&'a StAuthorization> {
    use time::format_description::well_known::Rfc3339;

    authorizations
        .iter()
        .enumerate()
        .filter(|(_, a)| existing_id.map(|e| a.id != e).unwrap_or(true))
        .max_by_key(|(idx, a)| {
            let ts = a
                .created_date
                .as_deref()
                .and_then(|s| OffsetDateTime::parse(s, &Rfc3339).ok());
            // (timestamp, idx) — both monotonically prefer newer/later.
            (ts, *idx)
        })
        .map(|(_, a)| a)
}

fn record_failure_locked(
    pool: &sqlx::PgPool,
    user_id: Option<Uuid>,
    error_code: &'static str,
    snaptrade_status: Option<u16>,
) {
    // Fire-and-forget; audit failures must never break the user-facing path.
    let pool = pool.clone();
    let event = audit::AuditEvent::new("broker.connect.failed");
    let mut data = json!({ "error_code": error_code });
    if let Some(s) = snaptrade_status {
        data["snaptrade_status"] = json!(s);
    }
    let event = match user_id {
        Some(uid) => event.user(uid).data(&data),
        None => event.data(&data),
    };
    // We can't await inside this sync helper; spawn a task on the global
    // runtime so the audit row gets written.
    tokio::spawn(async move {
        if let Err(err) = audit::record_event(&pool, event).await {
            tracing::warn!(error = %err, "audit broker.connect.failed write failed");
        }
    });
}

fn connection_failed_html() -> String {
    // Rendered when SnapTrade redirected the user to the callback but no
    // new authorization was created (e.g. user closed the broker login
    // mid-flow). HTTP 200 — a JSON 4xx would surface as a raw error to a
    // browser session that the user is staring at.
    r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="robots" content="noindex" />
    <meta name="viewport" content="width=device-width,initial-scale=1" />
    <title>Mizan Connect — Connection didn't complete</title>
    <style>
      body { margin: 0; font: 16px/1.5 ui-sans-serif, system-ui, sans-serif;
             color: #f5e6c8; background: #0a0a0a;
             display: grid; place-items: center; min-height: 100vh; }
      main { max-width: 28rem; padding: 2rem; text-align: center; }
      h1 { font-size: 1.25rem; margin: 0 0 .5rem;
           background: linear-gradient(135deg,#F5E6C8 0%,#D4A574 50%,#8B6F47 100%);
           -webkit-background-clip: text; background-clip: text; color: transparent; }
      p { color: #b8a98a; margin: 0; }
      .mark { font-size: 2.5rem; line-height: 1; margin-bottom: 1rem; }
    </style>
  </head>
  <body>
    <main>
      <div class="mark" aria-hidden="true">!</div>
      <h1>Connection didn't complete</h1>
      <p>Close this window and try again from Mizan.</p>
    </main>
  </body>
</html>"#
        .to_string()
}

fn success_html() -> String {
    // Plain, self-contained, no external assets. Closing the tab returns
    // control to Mizan desktop.
    r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="robots" content="noindex" />
    <meta name="viewport" content="width=device-width,initial-scale=1" />
    <title>Mizan Connect — Broker linked</title>
    <style>
      body { margin: 0; font: 16px/1.5 ui-sans-serif, system-ui, sans-serif;
             color: #f5e6c8; background: #0a0a0a;
             display: grid; place-items: center; min-height: 100vh; }
      main { max-width: 28rem; padding: 2rem; text-align: center; }
      h1 { font-size: 1.25rem; margin: 0 0 .5rem;
           background: linear-gradient(135deg,#F5E6C8 0%,#D4A574 50%,#8B6F47 100%);
           -webkit-background-clip: text; background-clip: text; color: transparent; }
      p { color: #b8a98a; margin: 0; }
      .tick { font-size: 2.5rem; line-height: 1; margin-bottom: 1rem; }
    </style>
  </head>
  <body>
    <main>
      <div class="tick" aria-hidden="true">✓</div>
      <h1>Broker linked</h1>
      <p>You can close this window and return to Mizan.</p>
    </main>
  </body>
</html>"#
        .to_string()
}

// ---------------------------------------------------------------------------
// GET /api/v1/sync/brokerage/connections
// ---------------------------------------------------------------------------

pub async fn list_connections(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<BrokerConnectionDto>>, AppError> {
    let row = match repository::fetch_active_for_user(state.db(), user.id).await? {
        Some(r) => r,
        None => return Ok(Json(vec![])),
    };
    let user_secret = state
        .encryption()
        .decrypt(&row.snaptrade_user_secret_encrypted)?;
    let auths = state
        .snaptrade()
        .list_authorizations(&row.snaptrade_user_id, &user_secret)
        .await?;
    let dtos = auths.into_iter().map(map_authorization).collect();
    Ok(Json(dtos))
}

fn map_authorization(a: StAuthorization) -> BrokerConnectionDto {
    let status = if a.disabled {
        "disconnected"
    } else {
        "connected"
    };
    BrokerConnectionDto {
        id: a.id,
        name: a.name,
        auth_type: a.auth_type,
        disabled: a.disabled,
        status,
        brokerage: a.brokerage.map(|b| BrokerageDto {
            id: b.id,
            slug: b.slug,
            name: b.name,
            display_name: b.display_name,
            aws_s3_logo_url: b.aws_s3_logo_url,
            aws_s3_square_logo_url: b.aws_s3_square_logo_url,
        }),
        created_date: a.created_date,
        updated_date: a.updated_date,
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/sync/brokerage/accounts
// ---------------------------------------------------------------------------

pub async fn list_accounts(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Json<Vec<BrokerAccountDto>>, AppError> {
    let row = match repository::fetch_active_for_user(state.db(), user.id).await? {
        Some(r) => r,
        None => return Ok(Json(vec![])),
    };
    let user_secret = state
        .encryption()
        .decrypt(&row.snaptrade_user_secret_encrypted)?;
    let accts = state
        .snaptrade()
        .list_accounts(&row.snaptrade_user_id, &user_secret)
        .await?;
    let dtos = accts.into_iter().map(map_account).collect();
    Ok(Json(dtos))
}

fn map_account(a: StAccount) -> BrokerAccountDto {
    BrokerAccountDto {
        id: a.id,
        brokerage_authorization: a.brokerage_authorization,
        name: a.name,
        number: a.number,
        institution_name: a.institution_name,
        status: a.status,
        raw_type: a.raw_type,
        balance: a.balance,
        sync_status: a.sync_status,
        is_paper: a.is_paper.unwrap_or(false),
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/sync/brokerage/accounts/:id/holdings
// ---------------------------------------------------------------------------

pub async fn account_holdings(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(account_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let row = repository::fetch_active_for_user(state.db(), user.id)
        .await?
        .ok_or_else(|| AppError::not_found("no active broker connection"))?;
    let user_secret = state
        .encryption()
        .decrypt(&row.snaptrade_user_secret_encrypted)?;
    let v = state
        .snaptrade()
        .account_holdings(&row.snaptrade_user_id, &user_secret, &account_id)
        .await?;
    Ok(Json(v))
}

// ---------------------------------------------------------------------------
// GET /api/v1/sync/brokerage/accounts/:id/activities
// ---------------------------------------------------------------------------

pub async fn account_activities(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(account_id): Path<String>,
    Query(q): Query<ActivitiesQuery>,
) -> Result<Json<Value>, AppError> {
    let row = repository::fetch_active_for_user(state.db(), user.id)
        .await?
        .ok_or_else(|| AppError::not_found("no active broker connection"))?;
    let user_secret = state
        .encryption()
        .decrypt(&row.snaptrade_user_secret_encrypted)?;

    let limit = q
        .limit
        .unwrap_or(ACTIVITIES_DEFAULT_LIMIT)
        .clamp(1, ACTIVITIES_MAX_LIMIT);
    let offset: u32 = q
        .cursor
        .as_deref()
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    let limit_s = limit.to_string();
    let offset_s = offset.to_string();

    let mut params: Vec<(&str, &str)> =
        vec![("limit", limit_s.as_str()), ("offset", offset_s.as_str())];
    if let Some(s) = q.start_date.as_deref() {
        params.push(("startDate", s));
    }
    if let Some(s) = q.end_date.as_deref() {
        params.push(("endDate", s));
    }

    let v = state
        .snaptrade()
        .account_activities(&row.snaptrade_user_id, &user_secret, &account_id, &params)
        .await?;
    Ok(Json(v))
}

// ---------------------------------------------------------------------------
// POST /api/v1/sync/brokerage/connections/:id/refresh
// ---------------------------------------------------------------------------

pub async fn refresh_connection(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(authorization_id): Path<String>,
) -> Result<Json<RefreshResponse>, AppError> {
    // The path param is SnapTrade's authorization id — that's what the
    // desktop receives via `/connections` and uses as the row identifier.
    // The local `broker_connections.id` UUID is never exposed to clients.
    let row =
        repository::fetch_by_authorization_id_for_user(state.db(), user.id, &authorization_id)
            .await?
            .ok_or_else(|| AppError::not_found("connection not found"))?;
    let auth_id = row
        .snaptrade_authorization_id
        .as_deref()
        .ok_or_else(|| AppError::bad_request("connection is not yet linked"))?;
    let user_secret = state
        .encryption()
        .decrypt(&row.snaptrade_user_secret_encrypted)?;

    state
        .snaptrade()
        .refresh_authorization(&row.snaptrade_user_id, &user_secret, auth_id)
        .await?;

    let _ = audit::record_event(
        state.db(),
        audit::AuditEvent::new("broker.refresh")
            .user(user.id)
            .data(&json!({ "authorization_id": auth_id })),
    )
    .await;

    Ok(Json(RefreshResponse { ok: true }))
}

#[derive(Debug, Serialize)]
pub struct RefreshResponse {
    pub ok: bool,
}

// ---------------------------------------------------------------------------
// DELETE /api/v1/sync/brokerage/connections/:id
// ---------------------------------------------------------------------------

pub async fn delete_connection(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Path(authorization_id): Path<String>,
) -> Result<Json<RefreshResponse>, AppError> {
    // Path param is SnapTrade's authorization id (the value the desktop
    // receives via `/connections` and uses as the row identity). The
    // local `broker_connections.id` UUID is server-internal and never
    // appears in the public API surface, so handlers MUST resolve via
    // the SnapTrade id. Earlier versions accepted `Path<Uuid>` and
    // looked up by local id, which produced a 404 on every disconnect
    // because the two ID spaces don't match.
    let row =
        repository::fetch_by_authorization_id_for_user(state.db(), user.id, &authorization_id)
            .await?
            .ok_or_else(|| AppError::not_found("connection not found"))?;

    // Already-disabled rows skip the SnapTrade call so a repeat DELETE is
    // a pure no-op locally (no upstream traffic, no audit duplication).
    if !row.disabled {
        if let Some(auth_id) = row.snaptrade_authorization_id.as_deref() {
            let user_secret = state
                .encryption()
                .decrypt(&row.snaptrade_user_secret_encrypted)?;
            // remove_authorization is idempotent — 404 from upstream maps to Ok(()).
            state
                .snaptrade()
                .remove_authorization(&row.snaptrade_user_id, &user_secret, auth_id)
                .await?;
        }
    }

    // soft_delete still uses the local row id — we resolved it from the
    // authorization-id lookup above.
    let _ = repository::soft_delete(state.db(), user.id, row.id).await?;
    let _ = audit::record_event(
        state.db(),
        audit::AuditEvent::new("broker.disconnect")
            .user(user.id)
            .data(&json!({ "authorization_id": row.snaptrade_authorization_id })),
    )
    .await;

    Ok(Json(RefreshResponse { ok: true }))
}

// Use the encryption error path through AppError.
impl From<crate::snaptrade::encryption::EncryptionError> for AppError {
    fn from(err: crate::snaptrade::encryption::EncryptionError) -> Self {
        tracing::error!(error = %err, "broker secret decryption failed");
        AppError::internal("broker integration error")
    }
}
