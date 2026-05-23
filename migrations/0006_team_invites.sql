-- Mizan Connect — Team invites (M5.3)
-- ----------------------------------------------------------------------------
-- Owner-only invite issuance + redemption table. Each row is one outstanding
-- invitation token that lets a (matching email) user join a team at a
-- specified role within the TTL window.
--
-- Token shape: opaque random 32-byte hex (64 chars). We could JWT-sign these
-- but a DB-backed token is dead-simple to revoke (delete the row), audit
-- (joinedAt + redeemedAt), and rate-limit (lookup by team_id).
--
-- Forward-only, idempotent — safe to re-run against a clean DB.

BEGIN;

CREATE TABLE team_invites (
    token        TEXT PRIMARY KEY,
    team_id      UUID NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    invited_by   UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Lowercased email; redeemer must match (case-insensitive) on accept.
    email        TEXT NOT NULL,
    role         TEXT NOT NULL CHECK (role IN ('advisor', 'viewer')),
    -- NULL until accepted. Once set, the invite is consumed.
    redeemed_at  TIMESTAMPTZ,
    redeemed_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at   TIMESTAMPTZ NOT NULL
);

-- For listing outstanding invites on the team admin page.
CREATE INDEX idx_team_invites_team
    ON team_invites(team_id)
    WHERE redeemed_at IS NULL;

-- Case-insensitive email lookup so we can show a user "you have an invite
-- waiting" hint at sign-in if they happen to be the invitee.
CREATE INDEX idx_team_invites_email
    ON team_invites(LOWER(email))
    WHERE redeemed_at IS NULL;

COMMIT;
