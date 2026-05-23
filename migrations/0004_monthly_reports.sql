-- Mizan Connect — M3.6 — Monthly AI wealth reports.
-- Forward-only.
--
-- The reports_cron job fires at 03:00 UTC on the 1st of each month and
-- enqueues one row per active subscription (status='pending'). A worker task
-- consumes pending rows, calls the AI proxy with kind=monthly_report, and
-- updates the row with the rendered markdown + status. On-demand
-- regeneration uses the same table — the API handler inserts a fresh row
-- with status='pending' and the worker picks it up just like a cron entry.

CREATE TABLE monthly_reports (
    id              UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    period_start    DATE NOT NULL,
    period_end      DATE NOT NULL,
    -- Markdown body the desktop renders. NULL while status='pending'.
    summary_md      TEXT,
    -- Worker fills in for audit / pricing parity with usage_ledger entries.
    model           TEXT,
    credits_charged INTEGER NOT NULL DEFAULT 0,
    -- 'pending' once enqueued; 'succeeded' / 'failed' once the worker
    -- finishes. Failures stay so the worker doesn't retry forever; the user
    -- (or admin) can request regen on-demand.
    status          TEXT NOT NULL CHECK (status IN ('pending', 'succeeded', 'failed')),
    error           TEXT,
    requested_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    generated_at    TIMESTAMPTZ,
    CONSTRAINT uq_monthly_report_per_period UNIQUE (user_id, period_start)
);
CREATE INDEX idx_monthly_reports_user_time ON monthly_reports(user_id, period_start DESC);
CREATE INDEX idx_monthly_reports_pending ON monthly_reports(status, requested_at)
    WHERE status = 'pending';
