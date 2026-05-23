-- Mizan Connect — Chunk 4: Stripe billing + usage ledger
-- Forward-only. See .claude/skills/add-database-migration.md for rules.

-- ---------------------------------------------------------------------------
-- Convert subscription_tier from ENUM to TEXT-with-CHECK so we can add new
-- slugs (`pro`, `enterprise`) without `ALTER TYPE ADD VALUE` — which can't
-- run inside a transaction in older Postgres and is awkward under
-- sqlx::migrate!'s default tx wrapping. Existing rows (`basic` / `essentials`
-- / `duo` / `plus`) continue to round-trip unchanged as TEXT.
-- ---------------------------------------------------------------------------
ALTER TABLE subscriptions ALTER COLUMN tier TYPE TEXT USING tier::TEXT;
DROP TYPE subscription_tier;
ALTER TABLE subscriptions ADD CONSTRAINT subscriptions_tier_check
    CHECK (tier IN ('basic', 'essentials', 'duo', 'plus', 'pro', 'enterprise'));

-- ---------------------------------------------------------------------------
-- AI credit accounting on the subscription row.
-- `ai_credits_used` resets to 0 on each invoice.paid webhook (period reset).
-- `ai_credits_period_start` mirrors the Stripe invoice period anchor so the
-- /v1/me response can surface the next reset time to the desktop client.
-- ---------------------------------------------------------------------------
ALTER TABLE subscriptions
    ADD COLUMN IF NOT EXISTS ai_credits_used INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS ai_credits_period_start TIMESTAMPTZ;

-- ---------------------------------------------------------------------------
-- usage_ledger — append-only spend log.
-- Every metered action (AI reply, broker poll, CSV import, market refresh)
-- writes one row. AI replies also bump subscriptions.ai_credits_used in the
-- same transaction; that aggregate is what we expose to the client, the
-- ledger is the audit trail.
-- ---------------------------------------------------------------------------
CREATE TYPE usage_metric AS ENUM (
    'ai_reply',
    'broker_poll',
    'csv_intel',
    'market_refresh'
);

CREATE TABLE usage_ledger (
    id              BIGSERIAL PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    metric          usage_metric NOT NULL,
    units           INTEGER NOT NULL CHECK (units >= 0),
    cost_credits    INTEGER NOT NULL DEFAULT 0 CHECK (cost_credits >= 0),
    model           TEXT,
    kind            TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_usage_ledger_user_time ON usage_ledger(user_id, created_at DESC);

-- ---------------------------------------------------------------------------
-- stripe_events — webhook idempotency.
-- Every event id we successfully process is recorded here in the same tx as
-- the side effect, so a replay short-circuits before any DB mutation.
-- ---------------------------------------------------------------------------
CREATE TABLE stripe_events (
    stripe_event_id TEXT PRIMARY KEY,
    event_type      TEXT NOT NULL,
    processed_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
