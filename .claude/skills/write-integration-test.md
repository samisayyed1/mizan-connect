# Skill: Write an integration test

Use when: adding tests in `tests/`.

## Pattern
```rust
mod common;
use common::TestApp;

#[tokio::test]
async fn me_returns_user_when_authenticated() {
    let app = TestApp::spawn().await;
    let token = app.mint_test_jwt("test-user-1", "test@example.com").await;

    let res = app.client
        .get(format!("{}/v1/me", app.address))
        .bearer_auth(token)
        .send()
        .await
        .expect("request failed");

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("invalid json");
    assert_eq!(body["email"], "test@example.com");
}
```

## TestApp helper (lives in `tests/common/mod.rs`)
- Spins up a Postgres container via `testcontainers`.
- Runs migrations.
- Configures the app with a test JWT signing key (so we don't need real Supabase).
- Provides `mint_test_jwt(sub, email)` — signs a JWT we'll accept.
- For tests, the auth layer accepts JWTs signed with a test key when `APP_ENV=test` AND `MIZAN_TEST_JWT_SECRET` is set. **Production builds reject this path.**
- Returns `address` (e.g., `http://127.0.0.1:42189`) of the running test server.

## Coverage requirements
For every new endpoint:
- Happy path: valid auth + valid input → expected response.
- Auth failure: missing / expired / wrong-issuer JWT → 401.
- Validation failure: bad input → 400 with field errors.
- Authorization failure (when applicable): valid auth but wrong user → 403.
