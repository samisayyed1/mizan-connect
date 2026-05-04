-- Mizan Connect — SnapTrade integration (Chunk 3).
-- Forward-only. See .claude/skills/add-database-migration.md for rules.
-- Extends `broker_connections` from migration 0001 with the columns +
-- indexes the SnapTrade flow needs. No data migration — the table is
-- empty in production at this point.

ALTER TABLE broker_connections
    ADD COLUMN IF NOT EXISTS connection_type TEXT NOT NULL DEFAULT 'snaptrade',
    ADD COLUMN IF NOT EXISTS broker_slug TEXT,
    ADD COLUMN IF NOT EXISTS account_count INT NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS disabled BOOLEAN NOT NULL DEFAULT FALSE;

-- One active SnapTrade connection per user. Multiple historical/inactive
-- rows allowed (e.g. user disconnects then reconnects).
CREATE UNIQUE INDEX IF NOT EXISTS uq_broker_conn_user_snaptrade
    ON broker_connections(user_id)
    WHERE connection_type = 'snaptrade' AND is_active = TRUE;

-- SnapTrade `userId` is unique across our tenancy. Two of our users sharing a
-- SnapTrade userId would let either of them act on the other's connection.
CREATE UNIQUE INDEX IF NOT EXISTS uq_broker_conn_snaptrade_uid
    ON broker_connections(snaptrade_user_id)
    WHERE connection_type = 'snaptrade';

-- Lookup-by-authorization for the callback handler and disconnect flow.
CREATE INDEX IF NOT EXISTS idx_broker_conn_authorization
    ON broker_connections(snaptrade_authorization_id);
