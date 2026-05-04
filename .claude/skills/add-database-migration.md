# Skill: Add a database migration

Use when: any schema change.

## Steps
1. Create file `migrations/<NNNN>_<short_name>.sql` (4-digit zero-padded; increment from latest).
2. Write the forward migration only. SQLx doesn't run down migrations in prod — design forward-only.
3. Use `IF NOT EXISTS` / `IF EXISTS` for idempotency where it doesn't compromise correctness.
4. Wrap multi-statement migrations conceptually safely; remember Postgres DDL inside a transaction.
5. Test locally:
   ```bash
   make migrate
   ```
6. Re-run `cargo sqlx prepare` to refresh `sqlx-data.json`.
7. Commit migration file + updated `sqlx-data.json` together.

## Rules
- Never edit a migration file after it has been pushed to `main`. Add a new one instead.
- Never `DROP TABLE` without explicit user approval.
- Always add indexes in the same migration as the column they index, unless the table is huge (then plan a separate migration with `CONCURRENTLY`).
- Use `TIMESTAMPTZ`, never `TIMESTAMP`.
- Money columns: `BIGINT` (cents) or `NUMERIC(20, 4)` — never `REAL`/`DOUBLE PRECISION`.
- Use `ON DELETE CASCADE` for child rows owned by a user. Use `ON DELETE SET NULL` for audit references.

## Naming
- Tables: plural snake_case (`users`, `broker_connections`).
- Columns: snake_case.
- Indexes: `idx_<table>_<columns>` for non-unique, `uq_<table>_<columns>` for unique.
- Foreign keys: `fk_<table>_<column>`.
