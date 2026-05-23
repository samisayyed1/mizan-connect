-- Mizan Connect — Chunk 4: Stripe billing + usage ledger
-- Forward-only. See .claude/skills/add-database-migration.md for rules.

-- ---------------------------------------------------------------------------
-- Extend subscription_tier ENUM with the product manual's slugs.
-- Pre-existing values (basic/essentials/duo/plus) stay valid; code maps
-- legacy slugs to the "pro" matrix until the cloud cuts over fully.
-- ALTER TYPE ... ADD VALUE is idempotent in modern Postgres via IF NOT EXISTS.
-- ---------------------------------------------------------------------------
ALTER TYPE subscription_tier ADD VALUE IF NOT EXISTS 'pro';
ALTER TYPE subscription_tier ADD VALUE IF NOT EXISTS 'enterprise';

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
