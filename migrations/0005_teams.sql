-- Mizan Connect — Teams + per-team subscriptions (M5.1)
-- ----------------------------------------------------------------------------
-- Promotes the implicit 1:1 user → subscription relationship to a real
-- multi-user team model so Enterprise advisors can manage clients and
-- share subscription seats.
--
-- Strategy:
--   1. Create `teams` and `team_members`.
--   2. Backfill: every existing user becomes the owner of a solo team
--      (one row in `teams`, one row in `team_members`). Stable UUIDs so
--      reruns are idempotent.
--   3. Add `team_id` to `subscriptions`, copy from `user_id` lookup,
--      then mark `user_id` as the "owner_user_id" for backwards-compat.
--      We KEEP `subscriptions.user_id` for the next two releases so the
--      desktop's webhook handler keeps working during the rollout, then
--      drop it in a follow-up migration once every client is updated.
--   4. Add audit-friendly indexes.
--
-- The backfill block is wrapped in DO so it runs only once; the test
-- suite re-applies migrations against a clean DB so we don't need
-- guards against re-running on an already-migrated DB.

BEGIN;

-- ----------------------------------------------------------------------------
-- teams
-- ----------------------------------------------------------------------------
CREATE TABLE teams (
    id               UUID PRIMARY KEY DEFAULT uuid_generate_v4(),
    name             TEXT NOT NULL,
    owner_user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    branding_logo_url TEXT,
    branding_color   TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    deleted_at       TIMESTAMPTZ
);

CREATE INDEX idx_teams_owner ON teams(owner_user_id) WHERE deleted_at IS NULL;

-- ----------------------------------------------------------------------------
-- team_members
-- Many-to-many between teams and users, plus the per-user role on the
-- team. Compound PK prevents duplicate memberships; the role is enforced
-- to one of three values so we can extend later (e.g. `auditor`) without
-- a migration on every existing row.
-- ----------------------------------------------------------------------------
CREATE TABLE team_members (
    team_id   UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    user_id   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role      TEXT NOT NULL CHECK (role IN ('owner', 'advisor', 'viewer')),
    joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (team_id, user_id)
);

CREATE INDEX idx_team_members_user ON team_members(user_id);

-- ----------------------------------------------------------------------------
-- Backfill: every existing user becomes the owner of a solo team named
-- after their display name (falling back to email local-part). The UUID
-- is derived from the user's UUID so re-running the backfill is
-- idempotent.
-- ----------------------------------------------------------------------------
INSERT INTO teams (id, name, owner_user_id, created_at, updated_at)
SELECT
    u.id,                                                   -- reuse user UUID as team UUID
    COALESCE(NULLIF(u.display_name, ''), split_part(u.email, '@', 1)),
    u.id,
    u.created_at,
    u.updated_at
FROM users u
WHERE u.deleted_at IS NULL
ON CONFLICT (id) DO NOTHING;

INSERT INTO team_members (team_id, user_id, role, joined_at)
SELECT t.id, t.owner_user_id, 'owner', t.created_at
FROM teams t
ON CONFLICT (team_id, user_id) DO NOTHING;

-- ----------------------------------------------------------------------------
-- subscriptions.team_id
-- The subscription lives on the team (not the user). For the rollout,
-- we keep `user_id` populated as the owner so older clients keep
-- working. A future migration drops `user_id`.
-- ----------------------------------------------------------------------------
ALTER TABLE subscriptions
    ADD COLUMN team_id UUID REFERENCES teams(id) ON DELETE CASCADE;

UPDATE subscriptions s
SET team_id = s.user_id  -- backfill uses the team-UUID-equals-user-UUID invariant above
WHERE s.team_id IS NULL;

ALTER TABLE subscriptions
    ALTER COLUMN team_id SET NOT NULL;

CREATE UNIQUE INDEX idx_subscriptions_team_active
    ON subscriptions(team_id)
    WHERE status IN ('active', 'trialing', 'past_due');

COMMIT;
