# Changes

## Unreleased — SnapTrade callback fix

The SnapTrade portal redirect carries `?state=<jwt>&userId=<snaptrade-user-id>`
only — it does **not** include `authorizationId`. The callback used to
hard-fail with `400 missing authorizationId`, so every Chunk 3 broker
connection broke at the last step.

The handler at `src/snaptrade/handlers.rs::snaptrade_callback` now:

1. Verifies the state JWT (unchanged) and resolves the local user.
2. Cross-checks `userId` against the row's `snaptrade_user_id` (unchanged).
3. Calls `GET /authorizations` via the existing
   `SnaptradeClient::list_authorizations` to fetch the user's
   authorizations.
4. Filters out the row's current `snaptrade_authorization_id` (so a replay
   never re-marks).
5. Picks the newest remaining authorization by `created_date` (RFC 3339;
   missing/unparseable entries rank as oldest, ties broken by response
   order — see `pick_newest_authorization`).
6. Persists `authorization_id`, `broker_slug`, and `institution_name`
   from `StAuthorization.brokerage` (no longer from query params).
7. Renders the user-friendly "Connection didn't complete" 200 HTML page
   when no new authorization is found and the row has no existing
   authorization. Returning a JSON 4xx here would be jarring — SnapTrade
   redirected the user to this URL in their browser.

Audit log: `broker.connect.completed` `event_data.authorization_id` is the
resolved id from the API response (no schema change).

### Idempotency

The schema has a partial unique index `uq_broker_conn_user_snaptrade` on
`(user_id) WHERE connection_type = 'snaptrade' AND is_active = TRUE` —
one active SnapTrade row per user. `mark_completed` is `UPDATE … WHERE id`,
which on replay sets the same column to the same value (no-op write,
no audit re-emit because the candidate set filters out the existing id).
**No new migration was required**; the index on `snaptrade_authorization_id`
in `migrations/0002_snaptrade.sql` is a plain `INDEX`, not `UNIQUE`, but
the natural row-ownership-by-user constraint covers the idempotency case.

### Removed

- `CallbackQuery::authorization_id` (and its three rename/aliases).
- `CallbackQuery::brokerage`.
- `CallbackQuery::institution_name`.
- The `missing_authorization` failure code is no longer emitted; replaced
  by `list_authorizations_failed` (upstream API failure) and
  `no_new_authorization` (callback fired without a fresh authorization
  to record).

### Tests

`tests/snaptrade_test.rs`:

- `callback_rejects_tampered_state` and `callback_rejects_expired_state`
  drop the `&authorizationId=abc` URL param (handler ignores it).
- `callback_persists_authorization_and_is_idempotent` →
  `callback_resolves_authorization_via_list_and_is_idempotent`: now mocks
  `GET /authorizations`, hits the callback twice, asserts a single row
  with `broker_slug` + `institution_name` resolved from the API.
- New: `callback_renders_failure_page_when_no_authorizations_found` —
  empty authorizations list → 200 HTML containing "didn't complete";
  row stays pending.
- New: `callback_picks_newest_authorization_ignoring_stale` — two
  authorizations, picks the one with the latest `created_date`.
- `seed_completed_connection` helper updated to mock `/authorizations`
  with `up_to_n_times(1)` so it doesn't shadow per-test stubs.
