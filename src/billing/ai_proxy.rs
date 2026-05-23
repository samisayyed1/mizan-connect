//! Managed-AI proxy endpoint — `POST /v1/ai/chat`.
//!
//! Pre-call:
//!   - Verify the user has `managed_ai` entitlement.
//!   - Reserve the request's estimated cost; reject 402 if it would exceed the
//!     monthly cap (unless that cap is `UNLIMITED`).
//!
//! Forward:
//!   - Pass the request through to upstream OpenAI using the cloud-owned key.
//!     Same JSON schema as `/v1/chat/completions` — the desktop's `mizan`
//!     provider speaks it directly.
//!
//! Post-call:
//!   - Reconcile the *actual* token usage → credit charge via
//!     [`crate::billing::credits::credits_for_tokens`]; append a
//!     `usage_ledger` row + bump `subscriptions.ai_credits_used` atomically.
//!
//! Streaming SSE forwarding lands as a follow-up; this initial cut handles
//! the non-streaming JSON case (`stream: false`) — sufficient for the desktop
//! provider's `complete` path and 90% of usage.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::AuthenticatedUser;
use crate::billing::credits::{credits_for_tokens, RequestKind};
use crate::billing::entitlements::UNLIMITED;
use crate::billing::repository as billing_repo;
use crate::error::AppError;
use crate::state::AppState;

const OPENAI_BASE: &str = "https://api.openai.com";
const DEFAULT_MODEL: &str = "gpt-4o-mini";
const UPSTREAM_TIMEOUT_SECS: u64 = 60;

/// Request body. Camel/snake compatible — we pass extra fields through, so
/// any OpenAI chat-completion knob the client sets reaches upstream verbatim.
#[derive(Debug, Deserialize)]
pub struct AiChatRequest {
    #[serde(default)]
    pub model: Option<String>,
    /// `simple` / `analysis` / `csv_mapping` / `monthly_report` / `deep_report`.
    /// Sets the pre-call credit reservation; absent ⇒ `simple`.
    #[serde(default)]
    pub kind: Option<RequestKind>,
    /// Pass-through of every other OpenAI field (messages, tools, temperature, etc.).
    #[serde(flatten)]
    pub passthrough: Value,
}

/// Response. The desktop reads `choices` + `usage` like any OpenAI client.
#[derive(Debug, Serialize)]
pub struct AiChatResponse {
    pub model: String,
    pub choices: Value,
    pub usage: TokenUsage,
    pub mizan_credits: CreditCharge,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

#[derive(Debug, Serialize)]
pub struct CreditCharge {
    pub charged: i32,
    pub used: i32,
    pub monthly: i32,
}

pub async fn chat(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(req): Json<AiChatRequest>,
) -> Result<impl IntoResponse, AppError> {
    // ── Pre-call gate ──────────────────────────────────────────────────
    let billing = state
        .billing()
        .ok_or_else(|| AppError::not_implemented("billing/AI not configured"))?;
    let openai_key = billing.openai_key.as_ref().ok_or_else(|| {
        AppError::service_unavailable("managed AI is not configured on this server")
    })?;

    let sub = billing_repo::fetch_active(state.db(), user.id).await?;
    let (ent_status, ent_tier) = match &sub {
        Some(s) => (Some(s.status.as_str()), Some(s.tier.as_str())),
        None => (None, None),
    };
    let entitlements = crate::billing::entitlements::entitlements_for(ent_tier, ent_status);
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

    // ── Forward to OpenAI ──────────────────────────────────────────────
    let model = req.model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let mut upstream_body = req.passthrough.clone();
    if let Value::Object(map) = &mut upstream_body {
        map.insert("model".to_string(), Value::String(model.clone()));
        // We don't support streaming yet; force non-stream so we can read the
        // usage object directly and reconcile credits.
        map.insert("stream".to_string(), Value::Bool(false));
    }

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(UPSTREAM_TIMEOUT_SECS))
        .build()
        .map_err(|e| AppError::internal("AI proxy http client").with_source(e))?;

    let resp = http
        .post(format!("{}/v1/chat/completions", OPENAI_BASE))
        .bearer_auth(openai_key.expose_secret())
        .json(&upstream_body)
        .send()
        .await
        .map_err(|e| AppError::service_unavailable("upstream AI unreachable").with_source(e))?;
    let status = resp.status();
    let raw = resp
        .text()
        .await
        .map_err(|e| AppError::service_unavailable("upstream AI read failed").with_source(e))?;
    if !status.is_success() {
        tracing::warn!(upstream_status = status.as_u16(), body = %raw, "upstream AI error");
        return Err(AppError::service_unavailable(
            "upstream AI returned an error",
        ));
    }
    let upstream: Value = serde_json::from_str(&raw).map_err(|e| {
        AppError::service_unavailable("malformed upstream AI response").with_source(e)
    })?;

    // ── Reconcile ──────────────────────────────────────────────────────
    let usage = upstream
        .get("usage")
        .cloned()
        .and_then(|v| serde_json::from_value::<TokenUsage>(v).ok())
        .unwrap_or(TokenUsage {
            prompt_tokens: 0,
            completion_tokens: 0,
            total_tokens: 0,
        });
    let charged = credits_for_tokens(usage.prompt_tokens, usage.completion_tokens);

    let mut tx = state.db().begin().await?;
    billing_repo::record_usage(
        &mut tx,
        user.id,
        "ai_reply",
        usage.total_tokens as i32,
        charged,
        Some(&model),
        Some(match kind {
            RequestKind::Simple => "simple",
            RequestKind::Analysis => "analysis",
            RequestKind::CsvMapping => "csv_mapping",
            RequestKind::MonthlyReport => "monthly_report",
            RequestKind::DeepReport => "deep_report",
        }),
    )
    .await?;
    let new_used = if sub.is_some() {
        billing_repo::add_ai_credits_used(&mut tx, user.id, charged).await?
    } else {
        // No paid subscription row → user is on Free (entitlements.managed_ai
        // would have rejected above). Should be unreachable.
        used + charged
    };
    tx.commit().await?;

    let body = json!({
        "model": model,
        "choices": upstream.get("choices").cloned().unwrap_or(json!([])),
        "usage": usage,
        "mizan_credits": {
            "charged": charged,
            "used": new_used,
            "monthly": monthly,
        }
    });
    Ok((StatusCode::OK, Json(body)))
}
