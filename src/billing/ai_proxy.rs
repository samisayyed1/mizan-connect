//! Managed-AI proxy endpoint — `POST /v1/ai/chat`.
//!
//! **SSE streaming**: the client (the desktop's `mizan` provider) opens an
//! event-stream and receives each upstream chunk as one `data: {...}` SSE
//! event. The proxy re-frames the OpenAI stream into a clean event-stream:
//!   - non-`data:` upstream lines (comments, blank) are dropped;
//!   - `data: {...}` deltas are forwarded verbatim (so existing OpenAI
//!     client libraries on the desktop side just work);
//!   - `data: [DONE]` is forwarded as the terminator;
//!   - the final upstream chunk carries `usage` (requested via
//!     `stream_options.include_usage = true`) → we extract it, then in the
//!     same task close the stream, write a `usage_ledger` row, and bump
//!     `subscriptions.ai_credits_used` atomically.
//!
//! Pre-flight gates (entitlement + monthly cap) are identical to the
//! non-streaming version that shipped in M1.5; only the response path moves
//! to SSE.
//!
//! Concurrency model: a `tokio::spawn`ed task drives the upstream stream and
//! pushes SSE events into a bounded `mpsc::channel`. The handler returns an
//! `Sse<ReceiverStream<…>>` that drains the channel. When upstream closes,
//! the task writes the ledger and drops the sender — that closes the SSE
//! cleanly. Errors mid-stream are logged but don't bubble into the SSE
//! payload (the desktop sees a truncated stream — same UX as any network
//! drop, and the user can retry; we don't double-charge because the ledger
//! write only runs on the success path).

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures::stream::StreamExt;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::auth::AuthenticatedUser;
use crate::billing::credits::{credits_for_tokens, RequestKind};
use crate::billing::entitlements::UNLIMITED;
use crate::billing::repository as billing_repo;
use crate::error::AppError;
use crate::state::AppState;

const OPENAI_BASE: &str = "https://api.openai.com";
const DEFAULT_MODEL: &str = "gpt-4o-mini";
const UPSTREAM_TIMEOUT_SECS: u64 = 120;
const SSE_CHANNEL_CAPACITY: usize = 32;

#[derive(Debug, Deserialize)]
pub struct AiChatRequest {
    #[serde(default)]
    pub model: Option<String>,
    /// `simple` / `analysis` / `csv_mapping` / `monthly_report` / `deep_report`.
    /// Sets the pre-call credit reservation; absent ⇒ `simple`.
    #[serde(default)]
    pub kind: Option<RequestKind>,
    /// Pass-through of every other OpenAI field (messages, tools, temperature, …).
    #[serde(flatten)]
    pub passthrough: Value,
}

#[derive(Debug, Default, Clone, Copy, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

/// `POST /v1/ai/chat` — SSE streaming proxy.
///
/// Returns `200 OK` + `text/event-stream` on success; `403` if the user lacks
/// `managed_ai`; `422` if their monthly credit cap would be exceeded; `503`
/// if managed AI isn't configured on this server or the upstream errors at
/// the connection step.
pub async fn chat(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(req): Json<AiChatRequest>,
) -> Result<Response, AppError> {
    // ── Pre-flight: entitlement + cap reservation ──────────────────────
    let billing = state
        .billing()
        .ok_or_else(|| AppError::not_implemented("billing/AI not configured"))?;
    let openai_key = billing
        .openai_key
        .as_ref()
        .ok_or_else(|| {
            AppError::service_unavailable("managed AI is not configured on this server")
        })?
        .clone();

    let sub = billing_repo::fetch_active(state.db(), user.id).await?;
    let (tier, status) = match &sub {
        Some(s) => (Some(s.tier.as_str()), Some(s.status.as_str())),
        None => (None, None),
    };
    let entitlements = crate::billing::entitlements::entitlements_for(tier, status);
    if !entitlements.managed_ai {
        return Err(AppError::forbidden(
            "managed Mizan AI requires a subscription",
        ));
    }

    let kind = req.kind.unwrap_or(RequestKind::Simple);
    let estimated = kind.estimated_cost();
    let used = sub.as_ref().map(|s| s.ai_credits_used).unwrap_or(0);
    let monthly = entitlements.ai_credits_monthly;
    if monthly != UNLIMITED && used + estimated > monthly {
        return Err(AppError::new(
            crate::error::ErrorCode::UnprocessableEntity,
            "AI credit cap reached for this billing period",
        ));
    }

    // ── Build upstream request body ────────────────────────────────────
    let model = req.model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let mut upstream_body = req.passthrough.clone();
    if let Value::Object(map) = &mut upstream_body {
        map.insert("model".to_string(), Value::String(model.clone()));
        map.insert("stream".to_string(), Value::Bool(true));
        // include_usage gives us the token counts in the final chunk before
        // `[DONE]` so we can reconcile credits without re-tokenizing.
        map.insert(
            "stream_options".to_string(),
            json!({ "include_usage": true }),
        );
    }

    // ── Open the upstream connection ───────────────────────────────────
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(UPSTREAM_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::internal("AI proxy http client").with_source(e))?;
    let upstream_resp = http
        .post(format!("{}/v1/chat/completions", OPENAI_BASE))
        .bearer_auth(openai_key.expose_secret())
        .json(&upstream_body)
        .send()
        .await
        .map_err(|e| AppError::service_unavailable("upstream AI unreachable").with_source(e))?;
    let upstream_status = upstream_resp.status();
    if !upstream_status.is_success() {
        let body = upstream_resp.text().await.unwrap_or_default();
        tracing::warn!(upstream_status = upstream_status.as_u16(), body = %body, "upstream AI error");
        return Err(AppError::service_unavailable(
            "upstream AI returned an error",
        ));
    }

    // ── Spawn the streamer ─────────────────────────────────────────────
    let (tx_evt, rx_evt) = mpsc::channel::<Result<Event, Infallible>>(SSE_CHANNEL_CAPACITY);
    let db = state.db().clone();
    let user_id = user.id;
    let model_for_task = model.clone();
    let kind_str = match kind {
        RequestKind::Simple => "simple",
        RequestKind::Analysis => "analysis",
        RequestKind::CsvMapping => "csv_mapping",
        RequestKind::MonthlyReport => "monthly_report",
        RequestKind::DeepReport => "deep_report",
    };

    tokio::spawn(async move {
        run_stream(upstream_resp, tx_evt, db, user_id, model_for_task, kind_str).await;
    });

    // ── Return the SSE response ────────────────────────────────────────
    let stream = ReceiverStream::new(rx_evt);
    Ok((
        StatusCode::OK,
        Sse::new(stream).keep_alive(KeepAlive::default()),
    )
        .into_response())
}

/// Drives the upstream stream → SSE channel → ledger reconciliation.
///
/// Runs inside a `tokio::spawn`. Any error here is logged + ends the SSE;
/// the ledger writes (which spend the user's credits) only fire on the
/// success path so we never charge for a stream the user didn't receive.
async fn run_stream(
    upstream_resp: reqwest::Response,
    tx_evt: mpsc::Sender<Result<Event, Infallible>>,
    db: sqlx::PgPool,
    user_id: uuid::Uuid,
    model: String,
    kind: &'static str,
) {
    let mut buf = String::new();
    let mut usage = TokenUsage::default();
    let mut saw_done = false;
    let mut byte_stream = upstream_resp.bytes_stream();

    while let Some(chunk_res) = byte_stream.next().await {
        let chunk = match chunk_res {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "AI proxy upstream stream error");
                break;
            }
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete lines (OpenAI separates events with `\n\n` and
        // individual lines with `\n`; we treat each `\n`-terminated line
        // independently and let blank lines fall through harmlessly).
        while let Some(nl) = buf.find('\n') {
            let line = buf[..nl].trim_end_matches('\r').to_string();
            buf.replace_range(..=nl, "");

            let Some(payload) = line.strip_prefix("data: ") else {
                continue;
            };
            if payload == "[DONE]" {
                saw_done = true;
                let _ = tx_evt.send(Ok(Event::default().data("[DONE]"))).await;
                break;
            }

            // Try to extract usage if present — OpenAI puts it in the
            // last chunk before `[DONE]` when stream_options.include_usage.
            if let Ok(v) = serde_json::from_str::<Value>(payload) {
                if let Some(u) = v.get("usage").cloned() {
                    if let Ok(parsed) = serde_json::from_value::<TokenUsage>(u) {
                        usage = parsed;
                    }
                }
            }

            if tx_evt
                .send(Ok(Event::default().data(payload)))
                .await
                .is_err()
            {
                // Receiver dropped (client disconnected). Stop streaming
                // but DON'T charge — incomplete reply.
                tracing::info!("AI proxy: client disconnected mid-stream");
                return;
            }
        }
        if saw_done {
            break;
        }
    }

    // ── Reconcile on the success path ──────────────────────────────────
    if !saw_done {
        tracing::warn!("AI proxy: upstream closed without [DONE]; skipping charge");
        return;
    }

    let charged = credits_for_tokens(usage.prompt_tokens, usage.completion_tokens);
    match db.begin().await {
        Ok(mut tx) => {
            if let Err(e) = billing_repo::record_usage(
                &mut tx,
                user_id,
                "ai_reply",
                usage.total_tokens as i32,
                charged,
                Some(&model),
                Some(kind),
            )
            .await
            {
                tracing::error!(error = %e, "AI proxy: usage_ledger insert failed");
                let _ = tx.rollback().await;
                return;
            }
            if let Err(e) = billing_repo::add_ai_credits_used(&mut tx, user_id, charged).await {
                tracing::error!(error = %e, "AI proxy: ai_credits_used bump failed");
                let _ = tx.rollback().await;
                return;
            }
            if let Err(e) = tx.commit().await {
                tracing::error!(error = %e, "AI proxy: ledger commit failed");
            }
        }
        Err(e) => {
            tracing::error!(error = %e, "AI proxy: failed to open ledger tx");
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn token_usage_default_zero() {
        let u = TokenUsage::default();
        assert_eq!(u.total_tokens, 0);
        assert_eq!(credits_for_tokens(u.prompt_tokens, u.completion_tokens), 1);
        // floor
    }

    #[test]
    fn parsed_usage_drives_credit_cost() {
        let payload = r#"{"id":"chatcmpl-x","choices":[],"usage":{"prompt_tokens":500,"completion_tokens":1500,"total_tokens":2000}}"#;
        let v: Value = serde_json::from_str(payload).unwrap();
        let u: TokenUsage = serde_json::from_value(v.get("usage").unwrap().clone()).unwrap();
        assert_eq!(u.prompt_tokens, 500);
        assert_eq!(u.completion_tokens, 1500);
        // 500 prompt + 3*1500 completion = 5000 weighted → 5 credits.
        assert_eq!(credits_for_tokens(u.prompt_tokens, u.completion_tokens), 5);
    }
}
