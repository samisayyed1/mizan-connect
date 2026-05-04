# Skill: Add a new API endpoint

Use when: adding any HTTP route to Mizan Connect.

## Steps
1. Decide the route. Public health/ready: unversioned. Everything else: under `/v1/`.
2. Create or extend the appropriate feature module (`src/<feature>/`):
   - `model.rs` — domain types and DTOs
   - `repository.rs` — SQLx queries (use `query_as!` macros)
   - `handlers.rs` — Axum handler functions
   - `mod.rs` — re-exports + `pub fn router() -> Router<AppState>`
3. Define request DTO with `#[derive(Deserialize, validator::Validate)]`.
4. Define response DTO with `#[derive(Serialize)]` — explicit, never domain models.
5. Handler signature: `async fn handler(State(s): State<AppState>, user: AuthenticatedUser, Json(req): Json<ReqDto>) -> Result<Json<RespDto>, AppError>`.
6. Wire up in `src/server.rs` via `.nest("/v1/<feature>", <feature>::router())`.
7. Add at least 2 integration tests in `tests/<feature>_test.rs`: happy path, auth-failure path.
8. Run `make lint && make test` before stopping.

## Authentication
- Public endpoints: omit the `AuthenticatedUser` extractor.
- Authenticated endpoints: include `user: AuthenticatedUser` — extractor handles JWT verification + user upsert.
- Admin endpoints: not in Chunk 1. Will be added with role claim later.

## Error responses
Always return `Result<_, AppError>`. The `AppError` IntoResponse impl produces:
```json
{ "error": { "code": "machine_readable", "message": "human readable", "request_id": "uuid" } }
```
Never leak internal error details (DB errors, file paths, etc.) — log them via tracing.
