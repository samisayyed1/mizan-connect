-- Mizan Connect — initial schema (Chunk 1)
-- Forward-only. See .claude/skills/add-database-migration.md for rules.

CREATE EXTENSION IF NOT EXISTS "uuid-ossp";
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ---------------------------------------------------------------------------
-- users
-- Local mirror of Supabase auth.users, keyed by supabase_user_id.
-- Populated lazily on first authenticated request via upsert.
-- ---------------------------------------------------------------------------
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    supabase_user_id UUID NOT NULL UNIQUE,
    email TEXT NOT NULL,
    display_name TEXT,
    avatar_url TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at TIMESTAMPTZ
);
CREATE INDEX idx_users_supabase ON users(supabase_user_id) WHERE deleted_at IS NULL;
CREATE INDEX idx_users_email ON users(LOWER(email)) WHERE deleted_at IS NULL;

-- ---------------------------------------------------------------------------
-- subscriptions (Chunk 2: Stripe billing)
-- ---------------------------------------------------------------------------
CREATE TYPE subscription_tier AS ENUM ('basic', 'essentials', 'duo', 'plus');
CREATE TYPE subscription_status AS ENUM (
    'active', 'past_due', 'canceled', 'trialing',
    'incomplete', 'incomplete_expired', 'unpaid'
);

CREATE TABLE subscriptions (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    stripe_customer_id TEXT UNIQUE,
    stripe_subscription_id TEXT UNIQUE,
    tier subscription_tier NOT NULL,
    status subscription_status NOT NULL,
    current_period_start TIMESTAMPTZ,
    current_period_end TIMESTAMPTZ,
    cancel_at_period_end BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_subscriptions_user ON subscriptions(user_id);
CREATE UNIQUE INDEX idx_subscriptions_active_per_user
    ON subscriptions(user_id)
    WHERE status IN ('active', 'trialing', 'past_due');

-- ---------------------------------------------------------------------------
-- broker_connections (Chunk 3: SnapTrade)
-- snaptrade_user_secret_encrypted holds the SnapTrade userSecret encrypted
-- at rest with the application key (rotated quarterly).
-- ---------------------------------------------------------------------------
CREATE TABLE broker_connections (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    snaptrade_user_id TEXT NOT NULL,
    snaptrade_user_secret_encrypted BYTEA NOT NULL,
    snaptrade_authorization_id TEXT,
    institution_name TEXT,
    last_synced_at TIMESTAMPTZ,
    last_sync_status TEXT,
    last_sync_error TEXT,
    is_active BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_broker_connections_user
    ON broker_connections(user_id) WHERE is_active = TRUE;

-- ---------------------------------------------------------------------------
-- sync_jobs (background work queue)
-- ---------------------------------------------------------------------------
CREATE TYPE sync_job_status AS ENUM (
    'pending', 'running', 'succeeded', 'failed', 'dead_letter'
);
CREATE TABLE sync_jobs (
    id UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    broker_connection_id UUID REFERENCES broker_connections(id) ON DELETE CASCADE,
    job_type TEXT NOT NULL,
    status sync_job_status NOT NULL DEFAULT 'pending',
    attempt_count INT NOT NULL DEFAULT 0,
    error_message TEXT,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_sync_jobs_user_status
    ON sync_jobs(user_id, status, created_at DESC);
CREATE INDEX idx_sync_jobs_pending
    ON sync_jobs(created_at) WHERE status = 'pending';

-- ---------------------------------------------------------------------------
-- audit_log (security/admin events ONLY — never financial data)
-- BIGSERIAL: append-only ledger; user_id SET NULL on user deletion so the
-- record survives for audit purposes even when the user row is removed.
-- ---------------------------------------------------------------------------
CREATE TABLE audit_log (
    id BIGSERIAL PRIMARY KEY,
    user_id UUID REFERENCES users(id) ON DELETE SET NULL,
    event_type TEXT NOT NULL,
    event_data JSONB,
    ip_address INET,
    user_agent TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_audit_log_user ON audit_log(user_id, created_at DESC);
CREATE INDEX idx_audit_log_event ON audit_log(event_type, created_at DESC);

-- ---------------------------------------------------------------------------
-- updated_at triggers
-- ---------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION set_updated_at() RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_users_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_subscriptions_updated_at
    BEFORE UPDATE ON subscriptions
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TRIGGER trg_broker_connections_updated_at
    BEFORE UPDATE ON broker_connections
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
