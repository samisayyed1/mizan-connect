//! Background scheduler + worker for monthly reports.
//!
//! Lifecycle:
//!   - Started once at app boot from `server::build_app` (or `main`).
//!   - The CRON-style task fires at **03:00 UTC on day 1 of every month** and
//!     enqueues a pending row per active subscription.
//!   - A worker loop ticks every 60s, drains up to N pending rows in bounded
//!     concurrency, calls the AI proxy, and writes results back.
//!
//! Both jobs share the same DB pool. On scheduler shutdown the JobScheduler is
//! dropped — pending work survives across restarts because state lives in the
//! `monthly_reports` table.

use sqlx::PgPool;
use tokio_cron_scheduler::{Job, JobScheduler};
use uuid::Uuid;

use super::reports;

const ENQUEUE_CRON: &str = "0 0 3 1 * *"; // sec=0, min=0, hour=3, day=1, month=*, weekday=*
const WORKER_TICK_SECS: u64 = 60;
/// Max concurrent AI proxy calls during a worker tick. Keeps OpenAI cost spikes
/// at the start of each month bounded — at 5×N pending, a full active-user
/// base of 1000 paying users drains over ~20 minutes.
const WORKER_PARALLELISM: usize = 5;

/// Start the monthly-report scheduler. Returns the scheduler handle so the
/// caller can shut it down on signal (drop = graceful shutdown).
pub async fn start(pool: PgPool) -> Result<JobScheduler, anyhow::Error> {
    let scheduler = JobScheduler::new().await?;

    // Job 1 — first-of-month enqueue.
    let enqueue_pool = pool.clone();
    let enqueue_job = Job::new_async(ENQUEUE_CRON, move |_uuid, _l| {
        let pool = enqueue_pool.clone();
        Box::pin(async move {
            if let Err(e) = run_enqueue(&pool).await {
                tracing::error!(error = %e, "monthly_reports enqueue failed");
            }
        })
    })?;
    scheduler.add(enqueue_job).await?;

    // Job 2 — worker tick. Cron `*/60 * * * * *` = every 60 seconds.
    let worker_pool = pool.clone();
    let worker_job = Job::new_repeated_async(
        std::time::Duration::from_secs(WORKER_TICK_SECS),
        move |_uuid, _l| {
            let pool = worker_pool.clone();
            Box::pin(async move {
                if let Err(e) = run_worker_tick(&pool).await {
                    tracing::error!(error = %e, "monthly_reports worker tick failed");
                }
            })
        },
    )?;
    scheduler.add(worker_job).await?;

    scheduler.start().await?;
    tracing::info!(
        "monthly_reports scheduler started (enqueue at {ENQUEUE_CRON}, worker every {WORKER_TICK_SECS}s)"
    );
    Ok(scheduler)
}

/// Enqueue a `pending` report row for every active subscriber for last month.
/// Idempotent — the unique constraint on `(user_id, period_start)` makes
/// retries safe.
async fn run_enqueue(pool: &PgPool) -> Result<(), anyhow::Error> {
    let today = time::OffsetDateTime::now_utc().date();
    let (start, end) = reports::previous_month_window(today);

    let users: Vec<(Uuid,)> = sqlx::query_as(
        r#"
        SELECT user_id FROM subscriptions
         WHERE status IN ('active', 'trialing')
        "#,
    )
    .fetch_all(pool)
    .await?;

    tracing::info!(
        candidates = users.len(),
        period_start = %start,
        "monthly_reports enqueue starting",
    );

    let mut inserted = 0;
    for (user_id,) in users {
        match reports::enqueue_pending(pool, user_id, start, end).await {
            Ok(true) => inserted += 1,
            Ok(false) => {} // already enqueued — fine
            Err(e) => tracing::warn!(user_id = %user_id, error = %e, "enqueue failed for user"),
        }
    }
    tracing::info!(inserted, "monthly_reports enqueue done");
    Ok(())
}

/// Drain up to N pending rows and mark them succeeded/failed. The actual AI
/// proxy call is stubbed for now — the production wiring lands once the
/// internal "service-to-service AI call" path exists (the AI proxy in this
/// repo today fronts the external-facing endpoint which needs an
/// `AuthenticatedUser`). Until then, this worker writes a placeholder summary
/// so the table + UI flow can be exercised end-to-end.
async fn run_worker_tick(pool: &PgPool) -> Result<(), anyhow::Error> {
    let pending = reports::fetch_pending(pool, WORKER_PARALLELISM as i64).await?;
    if pending.is_empty() {
        return Ok(());
    }

    tracing::info!(count = pending.len(), "monthly_reports worker draining");

    // Process in parallel up to the cap.
    let tasks = pending.into_iter().map(|row| {
        let pool = pool.clone();
        tokio::spawn(async move { generate_one(pool, row).await })
    });
    for t in tasks {
        if let Err(e) = t.await {
            tracing::error!(error = %e, "worker task join failed");
        }
    }
    Ok(())
}

/// Drive a single report from pending → succeeded/failed.
///
/// Stubbed AI call: returns a deterministic placeholder summary. When the
/// internal AI proxy path lands (post-M3.6), swap this for the real call.
async fn generate_one(pool: PgPool, row: reports::MonthlyReport) {
    let month_label = format!(
        "{} {}",
        month_name(row.period_start.month() as u8),
        row.period_start.year(),
    );

    // ── Placeholder summary (deterministic, no LLM call) ─────────────────
    let summary = placeholder_summary(&month_label, row.user_id);

    if let Err(e) = reports::mark_succeeded(&pool, row.id, &summary, "placeholder-stub", 0).await {
        tracing::error!(report_id = %row.id, error = %e, "monthly_reports mark_succeeded failed");
        if let Err(e2) = reports::mark_failed(&pool, row.id, &format!("{e}")).await {
            tracing::error!(report_id = %row.id, error = %e2, "monthly_reports mark_failed failed");
        }
    }
}

fn month_name(m: u8) -> &'static str {
    match m {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "Unknown",
    }
}

fn placeholder_summary(month_label: &str, user_id: Uuid) -> String {
    format!(
        "## Your {month_label} wealth summary\n\n\
         _Coming soon._ The Mizan Connect cloud is wiring up the live AI-driven \
         monthly report generator — once that lands, this card will show your \
         net-worth delta, top movers, income received, goal progress, and \
         liability trend for the month, generated from your own data.\n\n\
         For now this is a placeholder so the report inbox and the rendering \
         path can be exercised end-to-end.\n\n\
         (Internal: user `{user_id}`.)\n\n\
         *This summary is generated from your own data — not investment advice.*"
    )
}
