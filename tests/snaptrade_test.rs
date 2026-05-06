//! Integration tests for the SnapTrade-backed brokerage endpoints.
//!
//! Every test fakes the SnapTrade upstream with a `wiremock` server.
//! No real SnapTrade credentials are ever used.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::time::Duration;

use common::TestApp;
use mizan_connect::snaptrade::state_token;
use secrecy::SecretString;
use serde_json::{json, Value};
use uuid::Uuid;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const VALID_SUB: &str = "44444444-4444-4444-4444-444444444444";
const SECOND_SUB: &str = "44444444-4444-4444-4444-444444444445";

async fn wiremock_app() -> (TestApp, MockServer) {
    let server = MockServer::start().await;
    let api_base = format!("{}/api/v1", server.uri());
    let app = TestApp::spawn_with_snaptrade(&api_base).await;
    (app, server)
}

// ---------------------------------------------------------------------------
// /login-portal
// ---------------------------------------------------------------------------

#[tokio::test]
async fn login_portal_requires_auth() {
    let (app, _server) = wiremock_app().await;
    let res = app
        .client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .json(&json!({}))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn login_portal_happy_path_returns_signed_state_url() {
    let (app, server) = wiremock_app().await;

    // SnapTrade registerUser
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/registerUser"))
        .and(header_exists("Signature"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": VALID_SUB,
            "userSecret": "fresh-snaptrade-user-secret",
        })))
        .expect(1)
        .mount(&server)
        .await;

    // SnapTrade login (returns the portal URL)
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/login"))
        .and(header_exists("Signature"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "redirectURI": "https://app.snaptrade.com/snapTrade/redeemToken?token=abc",
            "sessionId": "00000000-0000-0000-0000-000000000abc",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let token = app.mint_jwt(
        VALID_SUB,
        Some("portal@example.com"),
        None,
        Duration::from_secs(600),
    );

    let res = app
        .client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .bearer_auth(token)
        .json(&json!({}))
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.expect("json");
    let url = body["url"].as_str().expect("url");
    assert!(url.starts_with("https://app.snaptrade.com/"));
    assert!(body["expires_at"].as_str().is_some());
}

#[tokio::test]
async fn login_portal_rate_limit_kicks_in_at_eleventh_call() {
    let (app, server) = wiremock_app().await;
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/registerUser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": VALID_SUB,
            "userSecret": "rate-limit-secret",
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "redirectURI": "https://app.snaptrade.com/x",
        })))
        .mount(&server)
        .await;

    let token = app.mint_jwt(
        VALID_SUB,
        Some("rl@example.com"),
        None,
        Duration::from_secs(600),
    );

    for i in 0..10 {
        let res = app
            .client
            .post(format!(
                "{}/api/v1/sync/brokerage/login-portal",
                app.address
            ))
            .bearer_auth(&token)
            .json(&json!({}))
            .send()
            .await
            .expect("request");
        assert_eq!(res.status(), 200, "call #{i} should pass");
    }

    let res = app
        .client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 429);
    let body: Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "too_many_requests");
}

// ---------------------------------------------------------------------------
// /sync/snaptrade/callback (PUBLIC — bound by state JWT)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn callback_is_reachable_without_bearer_token() {
    // Verify the route is mounted outside the auth chain — calling it
    // without a Bearer header reaches the handler (not 401).
    let (app, _server) = wiremock_app().await;
    let res = app
        .client
        .get(format!("{}/api/v1/sync/snaptrade/callback", app.address))
        .send()
        .await
        .expect("request");
    // Missing state → 400 from the handler (NOT 401 from extractor).
    assert_eq!(res.status(), 400);
    let body: Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "bad_request");
}

#[tokio::test]
async fn callback_rejects_tampered_state() {
    let (app, _server) = wiremock_app().await;
    let res = app
        .client
        .get(format!(
            "{}/api/v1/sync/snaptrade/callback?state=not-a-real-jwt",
            app.address
        ))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn callback_rejects_expired_state() {
    use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
    let (app, _server) = wiremock_app().await;

    let claims = json!({
        "sub": Uuid::new_v4(),
        "nonce": Uuid::new_v4(),
        "iss": "mizan-snaptrade-state",
        "iat": time::OffsetDateTime::now_utc().unix_timestamp() - 7200,
        "exp": time::OffsetDateTime::now_utc().unix_timestamp() - 3600,
    });
    let secret = common::TEST_STATE_SECRET;
    let token = encode(
        &Header::new(Algorithm::HS256),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .expect("encode");

    let res = app
        .client
        .get(format!(
            "{}/api/v1/sync/snaptrade/callback?state={}",
            app.address, token
        ))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 400);
}

#[tokio::test]
async fn callback_resolves_authorization_via_list_and_is_idempotent() {
    let (app, server) = wiremock_app().await;

    // First, run a /login-portal to register the user and create the
    // pending broker_connections row.
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/registerUser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": VALID_SUB,
            "userSecret": "callback-secret",
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "redirectURI": "https://app.snaptrade.com/x",
        })))
        .mount(&server)
        .await;

    // The callback now calls GET /authorizations to find the new auth id.
    let auth_id = "auth-1234";
    Mock::given(method("GET"))
        .and(path("/api/v1/authorizations"))
        .and(header_exists("Signature"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "id": auth_id,
                "name": "My Robinhood",
                "type": "trade",
                "disabled": false,
                "created_date": "2026-01-02T03:04:05Z",
                "updated_date": "2026-01-02T03:04:05Z",
                "brokerage": {
                    "id": "brk-1",
                    "slug": "ROBINHOOD",
                    "name": "Robinhood",
                    "display_name": "Robinhood",
                    "enabled": true,
                }
            }
        ])))
        .mount(&server)
        .await;

    let token = app.mint_jwt(
        VALID_SUB,
        Some("cb@example.com"),
        None,
        Duration::from_secs(600),
    );
    app.client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .expect("init");

    // Look up our user_id and mint a state JWT for the callback.
    let user_row: (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind("cb@example.com")
        .fetch_one(&app.pool)
        .await
        .expect("user row");
    let user_id = user_row.0;
    let state = state_token::issue(
        user_id,
        &SecretString::from(String::from(common::TEST_STATE_SECRET)),
    )
    .expect("issue state");

    // SnapTrade redirects with state + userId, NOT authorizationId.
    let url = format!(
        "{}/api/v1/sync/snaptrade/callback?state={}&userId={}",
        app.address, state, VALID_SUB
    );
    let res = app.client.get(&url).send().await.expect("first call");
    assert_eq!(res.status(), 200);
    assert!(res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.starts_with("text/html"))
        .unwrap_or(false));

    // Second call (idempotent) → still 200, still single row.
    let res2 = app.client.get(&url).send().await.expect("second call");
    assert_eq!(res2.status(), 200);

    // Database: exactly one row with this authorization_id, and the
    // brokerage / institution columns were resolved from the API response.
    let row: (i64, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT COUNT(*) OVER (), broker_slug, institution_name \
         FROM broker_connections \
         WHERE user_id = $1 AND snaptrade_authorization_id = $2",
    )
    .bind(user_id)
    .bind(auth_id)
    .fetch_one(&app.pool)
    .await
    .expect("row");
    assert_eq!(row.0, 1, "expect exactly one matching row after replay");
    assert_eq!(row.1.as_deref(), Some("ROBINHOOD"));
    assert_eq!(row.2.as_deref(), Some("Robinhood"));
}

#[tokio::test]
async fn callback_renders_failure_page_when_no_authorizations_found() {
    let (app, server) = wiremock_app().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/registerUser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": VALID_SUB,
            "userSecret": "no-auths-secret",
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "redirectURI": "https://app.snaptrade.com/x",
        })))
        .mount(&server)
        .await;
    // SnapTrade hasn't recorded any authorization for this user.
    Mock::given(method("GET"))
        .and(path("/api/v1/authorizations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;

    let token = app.mint_jwt(
        VALID_SUB,
        Some("noauth@example.com"),
        None,
        Duration::from_secs(600),
    );
    app.client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .expect("init");

    let user_row: (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind("noauth@example.com")
        .fetch_one(&app.pool)
        .await
        .expect("user row");
    let state = state_token::issue(
        user_row.0,
        &SecretString::from(String::from(common::TEST_STATE_SECRET)),
    )
    .expect("issue state");

    let res = app
        .client
        .get(format!(
            "{}/api/v1/sync/snaptrade/callback?state={}&userId={}",
            app.address, state, VALID_SUB
        ))
        .send()
        .await
        .expect("cb");
    assert_eq!(res.status(), 200);
    assert!(res
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.starts_with("text/html"))
        .unwrap_or(false));
    let body = res.text().await.expect("text");
    assert!(
        body.contains("didn't complete"),
        "expected user-friendly failure HTML, got: {body}"
    );

    // Row stays pending (no authorization id).
    let auth: (Option<String>,) = sqlx::query_as(
        "SELECT snaptrade_authorization_id FROM broker_connections WHERE user_id = $1",
    )
    .bind(user_row.0)
    .fetch_one(&app.pool)
    .await
    .expect("row");
    assert_eq!(auth.0, None);
}

#[tokio::test]
async fn callback_picks_newest_authorization_ignoring_stale() {
    let (app, server) = wiremock_app().await;

    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/registerUser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": VALID_SUB,
            "userSecret": "newest-secret",
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "redirectURI": "https://app.snaptrade.com/x",
        })))
        .mount(&server)
        .await;
    // Two authorizations: a stale one (older) and a fresh one. Handler
    // must pick the latter by created_date.
    Mock::given(method("GET"))
        .and(path("/api/v1/authorizations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "id": "auth-stale",
                "disabled": false,
                "created_date": "2025-12-01T00:00:00Z",
                "updated_date": "2025-12-01T00:00:00Z",
                "brokerage": {
                    "id": "brk-1",
                    "slug": "FIDELITY",
                    "name": "Fidelity",
                    "enabled": true,
                }
            },
            {
                "id": "auth-fresh",
                "disabled": false,
                "created_date": "2026-05-01T00:00:00Z",
                "updated_date": "2026-05-01T00:00:00Z",
                "brokerage": {
                    "id": "brk-2",
                    "slug": "ROBINHOOD",
                    "name": "Robinhood",
                    "display_name": "Robinhood",
                    "enabled": true,
                }
            }
        ])))
        .mount(&server)
        .await;

    let token = app.mint_jwt(
        VALID_SUB,
        Some("newest@example.com"),
        None,
        Duration::from_secs(600),
    );
    app.client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .expect("init");

    let user_row: (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind("newest@example.com")
        .fetch_one(&app.pool)
        .await
        .expect("user row");
    let state = state_token::issue(
        user_row.0,
        &SecretString::from(String::from(common::TEST_STATE_SECRET)),
    )
    .expect("issue state");

    let res = app
        .client
        .get(format!(
            "{}/api/v1/sync/snaptrade/callback?state={}&userId={}",
            app.address, state, VALID_SUB
        ))
        .send()
        .await
        .expect("cb");
    assert_eq!(res.status(), 200);

    let row: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT snaptrade_authorization_id, broker_slug \
         FROM broker_connections WHERE user_id = $1",
    )
    .bind(user_row.0)
    .fetch_one(&app.pool)
    .await
    .expect("row");
    assert_eq!(row.0.as_deref(), Some("auth-fresh"));
    assert_eq!(row.1.as_deref(), Some("ROBINHOOD"));
}

// ---------------------------------------------------------------------------
// list_connections — wiremock asserts signed request was made
// ---------------------------------------------------------------------------

#[tokio::test]
async fn list_connections_returns_mapped_authorizations() {
    let (app, server) = wiremock_app().await;

    // Set up a completed connection for VALID_SUB by walking the
    // login-portal + callback path against wiremock.
    seed_completed_connection(&app, &server, VALID_SUB, "complete@example.com", "auth-aaa").await;

    Mock::given(method("GET"))
        .and(path("/api/v1/authorizations"))
        .and(header_exists("Signature"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "id": "auth-aaa",
                "name": "My Robinhood",
                "type": "trade",
                "disabled": false,
                "created_date": "2026-01-02T03:04:05Z",
                "updated_date": "2026-01-02T03:04:06Z",
                "brokerage": {
                    "id": "brk-1",
                    "slug": "ROBINHOOD",
                    "name": "Robinhood",
                    "display_name": "Robinhood",
                    "enabled": true,
                }
            }
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let token = app.mint_jwt(
        VALID_SUB,
        Some("complete@example.com"),
        None,
        Duration::from_secs(600),
    );
    let res = app
        .client
        .get(format!("{}/api/v1/sync/brokerage/connections", app.address))
        .bearer_auth(token)
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.expect("json");
    let arr = body.as_array().expect("array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], "auth-aaa");
    assert_eq!(arr[0]["brokerage"]["slug"], "ROBINHOOD");
    assert_eq!(arr[0]["status"], "connected");
}

#[tokio::test]
async fn list_connections_empty_when_no_active_row() {
    let (app, _server) = wiremock_app().await;
    let token = app.mint_jwt(
        SECOND_SUB,
        Some("noconn@example.com"),
        None,
        Duration::from_secs(600),
    );
    let res = app
        .client
        .get(format!("{}/api/v1/sync/brokerage/connections", app.address))
        .bearer_auth(token)
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 200);
    let body: Value = res.json().await.expect("json");
    assert_eq!(body, json!([]));
}

// ---------------------------------------------------------------------------
// disconnect — DELETE on SnapTrade + soft-delete locally, idempotent
// ---------------------------------------------------------------------------

#[tokio::test]
async fn disconnect_deletes_upstream_and_soft_deletes_locally() {
    // Path param is SnapTrade's authorization id (the value the desktop
    // gets back from `/connections`), NOT the local broker_connections.id
    // UUID. Earlier versions of the handler accepted Path<Uuid> and
    // looked up by the local id, which always 404'd in production
    // because clients only know the SnapTrade id.
    let (app, server) = wiremock_app().await;
    let authorization_id = "auth-del";
    let user_id = seed_completed_connection(
        &app,
        &server,
        VALID_SUB,
        "del@example.com",
        authorization_id,
    )
    .await;

    // Confirm the local row exists (used only to read post-state).
    let local_row: (Uuid,) =
        sqlx::query_as("SELECT id FROM broker_connections WHERE user_id = $1 AND is_active = TRUE")
            .bind(user_id)
            .fetch_one(&app.pool)
            .await
            .expect("row");

    Mock::given(method("DELETE"))
        .and(path(format!("/api/v1/authorizations/{authorization_id}")))
        .and(header_exists("Signature"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;

    let token = app.mint_jwt(
        VALID_SUB,
        Some("del@example.com"),
        None,
        Duration::from_secs(600),
    );
    let res = app
        .client
        .delete(format!(
            "{}/api/v1/sync/brokerage/connections/{authorization_id}",
            app.address
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 200);

    // Local row must now be inactive + disabled.
    let after: (bool, bool) =
        sqlx::query_as("SELECT is_active, disabled FROM broker_connections WHERE id = $1")
            .bind(local_row.0)
            .fetch_one(&app.pool)
            .await
            .expect("row");
    assert_eq!(after, (false, true));

    // Idempotency: a second DELETE with the same authorization id.
    // After soft-delete the row's still in the table (just is_active=false,
    // disabled=true) and the lookup is by snaptrade_authorization_id, so
    // the handler resolves the row again. The `if !row.disabled` guard
    // skips the upstream SnapTrade call, so this second DELETE is a no-op
    // locally and never hits the wiremock again — verifying idempotency.
    let res2 = app
        .client
        .delete(format!(
            "{}/api/v1/sync/brokerage/connections/{authorization_id}",
            app.address
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("request");
    assert_eq!(res2.status(), 200);
}

// ---------------------------------------------------------------------------
// Two users can't share SnapTrade userId — second insert returns 409
// (enforced by the partial unique index in 0002_snaptrade.sql)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_users_cannot_share_snaptrade_user_id() {
    let (app, server) = wiremock_app().await;

    // Both users get the SAME snaptrade userId from registerUser — this
    // simulates an attacker / misconfiguration where SnapTrade returns a
    // colliding userId. The DB unique index must reject the second.
    let shared_st_id = "shared-snaptrade-uid";
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/registerUser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": shared_st_id,
            "userSecret": "secret-1",
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "redirectURI": "https://app.snaptrade.com/x",
        })))
        .mount(&server)
        .await;

    let t1 = app.mint_jwt(
        VALID_SUB,
        Some("a@example.com"),
        None,
        Duration::from_secs(600),
    );
    let t2 = app.mint_jwt(
        SECOND_SUB,
        Some("b@example.com"),
        None,
        Duration::from_secs(600),
    );

    let r1 = app
        .client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .bearer_auth(&t1)
        .json(&json!({}))
        .send()
        .await
        .expect("first");
    assert_eq!(r1.status(), 200, "first login-portal succeeds");

    let r2 = app
        .client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .bearer_auth(&t2)
        .json(&json!({}))
        .send()
        .await
        .expect("second");
    // The unique-index violation surfaces as 409 Conflict via AppError.
    assert_eq!(r2.status(), 409);
    let body: Value = r2.json().await.expect("json");
    assert_eq!(body["error"]["code"], "conflict");
}

// ---------------------------------------------------------------------------
// helper: drive the login-portal + callback flow against wiremock so a
// later test can read a completed broker_connections row.
// ---------------------------------------------------------------------------
async fn seed_completed_connection(
    app: &TestApp,
    server: &MockServer,
    sub: &str,
    email: &str,
    authorization_id: &str,
) -> Uuid {
    // registerUser + login mocks (each test body adds its own list/get/etc).
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/registerUser"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "userId": sub,
            "userSecret": format!("secret-{authorization_id}"),
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/snapTrade/login"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "redirectURI": "https://app.snaptrade.com/x",
        })))
        .mount(server)
        .await;
    // The callback resolves the authorization via GET /authorizations.
    // Capped at one match so subsequent list_connections / list_accounts
    // calls in the test body fall through to per-test mocks.
    Mock::given(method("GET"))
        .and(path("/api/v1/authorizations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            {
                "id": authorization_id,
                "disabled": false,
                "created_date": "2026-01-01T00:00:00Z",
                "updated_date": "2026-01-01T00:00:00Z",
                "brokerage": {
                    "id": "brk-test",
                    "slug": "TEST",
                    "name": "Test Broker",
                    "enabled": true,
                }
            }
        ])))
        .up_to_n_times(1)
        .mount(server)
        .await;

    let token = app.mint_jwt(sub, Some(email), None, Duration::from_secs(600));
    app.client
        .post(format!(
            "{}/api/v1/sync/brokerage/login-portal",
            app.address
        ))
        .bearer_auth(&token)
        .json(&json!({}))
        .send()
        .await
        .expect("portal");

    let user_row: (Uuid,) = sqlx::query_as("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(&app.pool)
        .await
        .expect("user row");
    let user_id = user_row.0;
    let state = state_token::issue(
        user_id,
        &SecretString::from(String::from(common::TEST_STATE_SECRET)),
    )
    .expect("issue");
    let cb = format!(
        "{}/api/v1/sync/snaptrade/callback?state={}&userId={}",
        app.address, state, sub
    );
    app.client.get(&cb).send().await.expect("cb");
    user_id
}
