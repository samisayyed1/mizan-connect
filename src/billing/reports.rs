//! Monthly AI Wealth Report — generator + HTTP handlers.
//!
//! Flow:
//!   1. Cron (`reports_cron.rs`) fires on the 1st of each month at 03:00 UTC.
//!      It enqueues one `monthly_reports` row per active subscription with
//!      `status='pending'`.
//!   2. A worker loop drains pending rows in bounded concurrency, calls the
//!      AI proxy (`ai_proxy::run_chat_completion`) with a structured prompt,
//!      and writes back `status='succeeded'` + the rendered markdown, or
//!      `status='failed'` + an error string.
//!   3. The desktop fetches the user's report list via `GET /v1/reports/monthly`.
//!      Pro+ tier can request an on-demand regeneration via `POST` — same
//!      table, same worker loop.
//!
//! Charges flow through the existing `ai_proxy` → `usage_ledger` /
//! `ai_credits_used` path. We never persist a `monthly_reports` row with a
//! credit charge separately — the AI proxy already records the charge against
//! the same user; we just store the rendered markdown for retrieval.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use time::{Date, OffsetDateTime};
use uuid::Uuid;

use crate::auth::AuthenticatedUser;
use crate::error::AppError;
use crate::state::AppState;

/// One stored report row.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct MonthlyReport {
    pub id: Uuid,
    pub user_id: Uuid,
    pub period_start: Date,
    pub period_end: Date,
    pub summary_md: Option<String>,
    pub model: Option<String>,
    pub credits_charged: i32,
    /// `pending` / `succeeded` / `failed`.
    pub status: String,
    pub error: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub generated_at: Option<OffsetDateTime>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Repository
// ─────────────────────────────────────────────────────────────────────────────

/// Insert a pending report row. Returns `false` when a row already exists for
/// the period (the unique constraint catches reruns of the cron + manual
/// on-demand collisions; no-op + Ok is the correct behavior).
pub async fn enqueue_pending(
    pool: &PgPool,
    user_id: Uuid,
    period_start: Date,
    period_end: Date,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        r#"
        INSERT INTO monthly_reports (user_id, period_start, period_end, status)
        VALUES ($1, $2, $3, 'pending')
        ON CONFLICT (user_id, period_start) DO NOTHING
        "#,
    )
    .bind(user_id)
    .bind(period_start)
    .bind(period_end)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Drain up to `limit` pending rows, oldest first. Used by the worker loop.
pub async fn fetch_pending(pool: &PgPool, limit: i64) -> Result<Vec<MonthlyReport>, sqlx::Error> {
    sqlx::query_as::<_, MonthlyReport>(
        r#"
        SELECT id, user_id, period_start, period_end, summary_md, model,
               credits_charged, status, error, requested_at, generated_at
          FROM monthly_reports
         WHERE status = 'pending'
         ORDER BY requested_at ASC
         LIMIT $1
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await
}

/// User-facing list query. Cap at 24 to avoid runaway responses.
pub async fn list_for_user(
    pool: &PgPool,
    user_id: Uuid,
    limit: i64,
) -> Result<Vec<MonthlyReport>, sqlx::Error> {
    sqlx::query_as::<_, MonthlyReport>(
        r#"
        SELECT id, user_id, period_start, period_end, summary_md, model,
               credits_charged, status, error, requested_at, generated_at
          FROM monthly_reports
         WHERE user_id = $1
         ORDER BY period_start DESC
         LIMIT $2
        "#,
    )
    .bind(user_id)
    .bind(limit.clamp(1, 24))
    .fetch_all(pool)
    .await
}

pub async fn mark_succeeded(
    pool: &PgPool,
    id: Uuid,
    summary_md: &str,
    model: &str,
    credits: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE monthly_reports
           SET status = 'succeeded',
               summary_md = $2,
               model = $3,
               credits_charged = $4,
               generated_at = NOW(),
               error = NULL
         WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(summary_md)
    .bind(model)
    .bind(credits)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_failed(pool: &PgPool, id: Uuid, reason: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE monthly_reports
           SET status = 'failed',
               error = $2,
               generated_at = NOW()
         WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(reason)
    .execute(pool)
    .await?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Prompt builder (pure)
// ─────────────────────────────────────────────────────────────────────────────

/// Build the LLM prompt for a monthly report.
///
/// Pure function so we can table-test it without a network round trip. The
/// prompt is intentionally NARROW: it tells the LLM to summarize the data the
/// caller provides — no predictions, no buy/sell recommendations. The system
/// prompt's "not financial advice" guardrail (set in the desktop's
/// `system_prompt.txt` and the cloud-issued template here) reinforces this at
/// the model level.
pub fn build_prompt(month_label: &str, dataset_json: &str) -> Vec<serde_json::Value> {
    let system = format!(
        "You are Mizan AI. You are writing a one-page monthly wealth summary for {month_label}. \
         Output clean Markdown only — no preamble, no code fences. Sections (use `##` headings): \
         1) Net worth this month (delta vs last month, headline number), \
         2) Top movers (3-5 holdings/positions that changed the most, in either direction), \
         3) Income received (dividends + interest + rent — aggregate), \
         4) Goals progress (one line per active goal), \
         5) Liabilities trend (one short paragraph), \
         6) What changed about your data this month (optional — note new connections, sync gaps). \
         Rules: describe what happened, do NOT predict, do NOT recommend buying or selling, do NOT \
         editorialize beyond what's in the data. If a section has no data, write `_No data this \
         period._` and move on. End with the one-line footer: `*This summary is generated from \
         your own data — not investment advice.*`",
    );
    let user = format!(
        "Here is the user's data for {month_label} as JSON. Render the markdown report per the \
         system instructions.\n\n```json\n{dataset_json}\n```"
    );
    vec![
        serde_json::json!({"role": "system", "content": system}),
        serde_json::json!({"role": "user", "content": user}),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Public API surface (HTTP handlers)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// 1-24; defaults to 12.
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct ListResponse {
    pub reports: Vec<MonthlyReport>,
}

/// `GET /v1/reports/monthly` — list the caller's reports, newest first.
pub async fn list_monthly_reports(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListResponse>, AppError> {
    let reports = list_for_user(state.db(), user.id, q.limit.unwrap_or(12)).await?;
    Ok(Json(ListResponse { reports }))
}

/// `POST /v1/reports/monthly` — enqueue an on-demand regeneration for the
/// current period. Idempotent: a second call within the same calendar month
/// is a no-op (returns the existing row). Pro+ feature; we don't enforce the
/// tier here because the underlying AI proxy call already burns credits and
/// gates on `managed_ai`. The frontend renders the upsell at /reports.
pub async fn request_monthly_report(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<(StatusCode, Json<MonthlyReport>), AppError> {
    let now = OffsetDateTime::now_utc().date();
    // The "report period" is the previous full calendar month. So if today is
    // 2026-06-12 we report on 2026-05-01..2026-05-31.
    let (start, end) = previous_month_window(now);

    let inserted = enqueue_pending(state.db(), user.id, start, end).await?;
    let row = fetch_one_by_period(state.db(), user.id, start)
        .await?
        .ok_or_else(|| AppError::internal("report row vanished after enqueue"))?;

    let status = if inserted {
        StatusCode::ACCEPTED
    } else {
        StatusCode::OK
    };
    Ok((status, Json(row)))
}

async fn fetch_one_by_period(
    pool: &PgPool,
    user_id: Uuid,
    period_start: Date,
) -> Result<Option<MonthlyReport>, sqlx::Error> {
    sqlx::query_as::<_, MonthlyReport>(
        r#"
        SELECT id, user_id, period_start, period_end, summary_md, model,
               credits_charged, status, error, requested_at, generated_at
          FROM monthly_reports
         WHERE user_id = $1 AND period_start = $2
        "#,
    )
    .bind(user_id)
    .bind(period_start)
    .fetch_optional(pool)
    .await
}

/// Compute the first/last day of the calendar month immediately preceding
/// `today`. Pure — table-tested.
///
/// The `.expect()` calls below are infallible by construction (first-of-month
/// is always a valid `Date`, and we adjust the year when wrapping past
/// January). Clippy can't prove this; the allow + comment is the cleanest
/// stable answer.
#[allow(clippy::expect_used)]
pub fn previous_month_window(today: Date) -> (Date, Date) {
    let (year, month) = match today.month().previous() {
        // `Month::previous()` wraps; `time` doesn't have a built-in
        // "(year, month) - 1" so we adjust the year when wrapping past Jan.
        prev if today.month() as u8 == 1 => (today.year() - 1, prev),
        prev => (today.year(), prev),
    };
    let start = Date::from_calendar_date(year, month, 1).expect("valid first-of-month");
    // Last day of the month = first of next month minus 1 day. `time`'s
    // `replace_day(31)` would clamp, but the cleaner approach is to compute
    // the first of the month *after* `start` and subtract one day.
    let next_month_first = if start.month() as u8 == 12 {
        Date::from_calendar_date(start.year() + 1, time::Month::January, 1).expect("Jan 1 valid")
    } else {
        Date::from_calendar_date(start.year(), start.month().next(), 1)
            .expect("first of next month valid")
    };
    let end = next_month_first - time::Duration::days(1);
    (start, end)
}

// Type aliasing: we accept `Arc<PgPool>` shape in some call sites. Placed
// here (before `mod tests`) to satisfy clippy's items-after-test-module lint.
#[allow(dead_code)]
pub type Pool = Arc<PgPool>;

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use time::Month;

    #[test]
    fn previous_month_window_mid_year() {
        // 2026-06-12 → May 2026
        let (s, e) =
            previous_month_window(Date::from_calendar_date(2026, Month::June, 12).unwrap());
        assert_eq!(s, Date::from_calendar_date(2026, Month::May, 1).unwrap());
        assert_eq!(e, Date::from_calendar_date(2026, Month::May, 31).unwrap());
    }

    #[test]
    fn previous_month_window_january_wraps_year() {
        // 2026-01-05 → December 2025
        let (s, e) =
            previous_month_window(Date::from_calendar_date(2026, Month::January, 5).unwrap());
        assert_eq!(
            s,
            Date::from_calendar_date(2025, Month::December, 1).unwrap()
        );
        assert_eq!(
            e,
            Date::from_calendar_date(2025, Month::December, 31).unwrap()
        );
    }

    #[test]
    fn previous_month_window_february_after_leap_year() {
        // 2024-03-01 → Feb 2024 (leap year, 29 days)
        let (s, e) =
            previous_month_window(Date::from_calendar_date(2024, Month::March, 1).unwrap());
        assert_eq!(
            s,
            Date::from_calendar_date(2024, Month::February, 1).unwrap()
        );
        assert_eq!(
            e,
            Date::from_calendar_date(2024, Month::February, 29).unwrap()
        );
    }

    #[test]
    fn build_prompt_contains_guardrails() {
        let p = build_prompt("May 2026", "{\"netWorth\": 1234}");
        let combined = p
            .iter()
            .map(|m| m["content"].as_str().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(combined.contains("do NOT predict"));
        assert!(combined.contains("do NOT recommend"));
        assert!(combined.contains("not investment advice"));
    }

    #[test]
    fn build_prompt_passes_dataset_through() {
        let p = build_prompt("May 2026", r#"{"foo":42}"#);
        let user = p[1]["content"].as_str().unwrap();
        assert!(user.contains(r#""foo":42"#));
    }
}
