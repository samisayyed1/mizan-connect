//! SQL queries for subscription + usage state.
//!
//! All mutations of `subscriptions` go through here so the audit trail
//! (`ai_credits_used`, period anchors, status transitions) stays consistent
//! with the webhook handler.

use sqlx::{PgConnection, PgPool};
use time::OffsetDateTime;
use uuid::Uuid;

use super::credits::credits_for_tokens;

/// Row shape used by the /v1/me handler.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SubscriptionRow {
    pub user_id: Uuid,
    pub stripe_customer_id: Option<String>,
    pub stripe_subscription_id: Option<String>,
    /// Tier slug as stored in the ENUM. Cast to TEXT in the SELECT.
    pub tier: String,
    pub status: String,
    pub current_period_end: Option<OffsetDateTime>,
    pub cancel_at_period_end: bool,
    pub ai_credits_used: i32,
    pub ai_credits_period_start: Option<OffsetDateTime>,
}

/// Fetch the user's currently-billable subscription, if any.
///
/// "Currently billable" = the row covered by the partial unique index on
/// `(user_id) WHERE status IN ('active','trialing','past_due')`. Returns
/// `None` when the user has never subscribed (Free).
pub async fn fetch_active(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<SubscriptionRow>, sqlx::Error> {
    sqlx::query_as::<_, SubscriptionRow>(
        r#"
        SELECT user_id,
               stripe_customer_id,
               stripe_subscription_id,
               tier::TEXT  AS tier,
               status::TEXT AS status,
               current_period_end,
               cancel_at_period_end,
               ai_credits_used,
               ai_credits_period_start
          FROM subscriptions
         WHERE user_id = $1
           AND status IN ('active', 'trialing', 'past_due')
         ORDER BY updated_at DESC
         LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
}

/// Persist or update the user's Stripe customer id at checkout-create time
/// (before any subscription row exists).
pub async fn upsert_customer_stub(
    pool: &PgPool,
    user_id: Uuid,
    stripe_customer_id: &str,
) -> Result<(), sqlx::Error> {
    // We create a placeholder subscription row in 'incomplete' status so
    // the FK from later webhook upserts has somewhere to land, even before
    // checkout completes. The webhook then promotes status/tier/period_end.
    sqlx::query(
        r#"
        INSERT INTO subscriptions (user_id, stripe_customer_id, tier, status)
        VALUES ($1, $2, 'basic', 'incomplete')
        ON CONFLICT (stripe_customer_id) DO UPDATE
          SET updated_at = NOW()
        "#,
    )
    .bind(user_id)
    .bind(stripe_customer_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Look up the user's Stripe customer id (created on first checkout).
pub async fn fetch_customer_id(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        r#"
        SELECT stripe_customer_id
          FROM subscriptions
         WHERE user_id = $1
           AND stripe_customer_id IS NOT NULL
         ORDER BY updated_at DESC
         LIMIT 1
        "#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| r.0))
}

/// Webhook subscription upsert payload (subset of the Stripe object).
pub struct WebhookSubscriptionUpsert<'a> {
    pub user_id: Uuid,
    pub stripe_customer_id: &'a str,
    pub stripe_subscription_id: &'a str,
    pub tier: &'a str,
    pub status: &'a str,
    pub current_period_start: Option<OffsetDateTime>,
    pub current_period_end: Option<OffsetDateTime>,
    pub cancel_at_period_end: bool,
}

/// Upsert by `stripe_subscription_id`. Called from the webhook handler;
/// caller is responsible for opening the tx (so the `stripe_events`
/// idempotency row + this write commit together).
pub async fn upsert_from_webhook(
    tx: &mut PgConnection,
    p: WebhookSubscriptionUpsert<'_>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO subscriptions (
            user_id, stripe_customer_id, stripe_subscription_id,
            tier, status,
            current_period_start, current_period_end,
            cancel_at_period_end
        ) VALUES ($1, $2, $3, $4::subscription_tier, $5::subscription_status, $6, $7, $8)
        ON CONFLICT (stripe_subscription_id) DO UPDATE
          SET status               = EXCLUDED.status,
              tier                 = EXCLUDED.tier,
              current_period_start = EXCLUDED.current_period_start,
              current_period_end   = EXCLUDED.current_period_end,
              cancel_at_period_end = EXCLUDED.cancel_at_period_end,
              updated_at           = NOW()
        "#,
    )
    .bind(p.user_id)
    .bind(p.stripe_customer_id)
    .bind(p.stripe_subscription_id)
    .bind(p.tier)
    .bind(p.status)
    .bind(p.current_period_start)
    .bind(p.current_period_end)
    .bind(p.cancel_at_period_end)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Reset AI credits to 0 at the start of each invoice period. Called from the
/// `invoice.paid` webhook handler in the same tx as the event-id insert.
pub async fn reset_ai_credits(
    tx: &mut PgConnection,
    stripe_subscription_id: &str,
    period_start: OffsetDateTime,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE subscriptions
           SET ai_credits_used = 0,
               ai_credits_period_start = $2,
               updated_at = NOW()
         WHERE stripe_subscription_id = $1
        "#,
    )
    .bind(stripe_subscription_id)
    .bind(period_start)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Append a usage_ledger row.
pub async fn record_usage(
    tx: &mut PgConnection,
    user_id: Uuid,
    metric: &str,
    units: i32,
    cost_credits: i32,
    model: Option<&str>,
    kind: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO usage_ledger (user_id, metric, units, cost_credits, model, kind)
        VALUES ($1, $2::usage_metric, $3, $4, $5, $6)
        "#,
    )
    .bind(user_id)
    .bind(metric)
    .bind(units)
    .bind(cost_credits)
    .bind(model)
    .bind(kind)
    .execute(&mut *tx)
    .await?;
    Ok(())
}

/// Increment `ai_credits_used` by the row count; called after a managed-AI
/// reply with the *actual* token-derived credit charge. Returns the new total
/// so the caller can decide whether to throttle subsequent requests.
pub async fn add_ai_credits_used(
    tx: &mut PgConnection,
    user_id: Uuid,
    credits: i32,
) -> Result<i32, sqlx::Error> {
    let row: (i32,) = sqlx::query_as(
        r#"
        UPDATE subscriptions
           SET ai_credits_used = ai_credits_used + $2,
               updated_at = NOW()
         WHERE user_id = $1
           AND status IN ('active', 'trialing', 'past_due')
        RETURNING ai_credits_used
        "#,
    )
    .bind(user_id)
    .bind(credits)
    .fetch_one(&mut *tx)
    .await?;
    Ok(row.0)
}

/// Idempotency check for webhook events. Inserts the event id; returns
/// `false` if it already existed (caller should short-circuit).
pub async fn try_record_stripe_event(
    tx: &mut PgConnection,
    event_id: &str,
    event_type: &str,
) -> Result<bool, sqlx::Error> {
    let res = sqlx::query(
        r#"
        INSERT INTO stripe_events (stripe_event_id, event_type)
        VALUES ($1, $2)
        ON CONFLICT (stripe_event_id) DO NOTHING
        "#,
    )
    .bind(event_id)
    .bind(event_type)
    .execute(&mut *tx)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Convenience: credits to charge for an actual AI reply (token reconciliation
/// after streaming). Centralizes the dependency on the credit table so other
/// callers don't reimplement it.
pub fn credits_for_reply(prompt_tokens: u32, completion_tokens: u32) -> i32 {
    credits_for_tokens(prompt_tokens, completion_tokens)
}
