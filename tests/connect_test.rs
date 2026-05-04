//! `/api/v1/...` Chunk-1 stub-coverage tests.
//!
//! Verifies the desktop-facing path aliases and the 501 stub envelope.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
use std::time::Duration;

use common::TestApp;

const VALID_SUB: &str = "22222222-2222-2222-2222-222222222222";

// ---------------------------------------------------------------------------
// /api/v1/user/me — alias of /v1/me
// ---------------------------------------------------------------------------

#[tokio::test]
async fn user_me_alias_returns_same_dto_as_v1_me() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("alias@example.com"),
        Some("Alias Tester"),
        Duration::from_secs(600),
    );

    let alias: serde_json::Value = app
        .client
        .get(format!("{}/api/v1/user/me", app.address))
        .bearer_auth(&token)
        .send()
        .await
        .expect("alias request")
        .json()
        .await
        .expect("alias json");

    let legacy: serde_json::Value = app
        .client
        .get(format!("{}/v1/me", app.address))
        .bearer_auth(&token)
        .send()
        .await
        .expect("legacy request")
        .json()
        .await
        .expect("legacy json");

    assert_eq!(alias["id"], legacy["id"], "same local user id");
    assert_eq!(alias["supabase_user_id"], legacy["supabase_user_id"]);
    assert_eq!(alias["email"], "alias@example.com");
    assert_eq!(alias["display_name"], "Alias Tester");
}

#[tokio::test]
async fn user_me_alias_returns_401_without_token() {
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/api/v1/user/me", app.address))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "unauthorized");
}

// ---------------------------------------------------------------------------
// /api/v1/subscription/plans — 501 with or without auth
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_plans_returns_501_with_auth() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("plans@example.com"),
        None,
        Duration::from_secs(600),
    );

    let res = app
        .client
        .get(format!("{}/api/v1/subscription/plans", app.address))
        .bearer_auth(token)
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 501);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "not_implemented");
    let request_id = body["error"]["request_id"]
        .as_str()
        .expect("request_id present");
    assert!(!request_id.is_empty(), "request_id is non-empty");
}

#[tokio::test]
async fn subscription_plans_returns_501_without_auth() {
    // The desktop also hits this path anonymously
    // (see fetch_subscription_plans_public in crates/connect). Must not
    // 401 — both flows should land on the same 501.
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/api/v1/subscription/plans", app.address))
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 501, "no auth required for plans stub");
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "not_implemented");
}

// ---------------------------------------------------------------------------
// /api/v1/sync/brokerage/* — 401 without auth, 501 with
// ---------------------------------------------------------------------------

#[tokio::test]
async fn brokerage_connections_returns_401_without_auth() {
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/api/v1/sync/brokerage/connections", app.address))
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn brokerage_connections_returns_501_with_auth() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("broker@example.com"),
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

    assert_eq!(res.status(), 501);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "not_implemented");
    assert!(
        body["error"]["request_id"].as_str().is_some(),
        "request_id present"
    );
}

#[tokio::test]
async fn brokerage_accounts_returns_501_with_auth() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("accounts@example.com"),
        None,
        Duration::from_secs(600),
    );

    let res = app
        .client
        .get(format!("{}/api/v1/sync/brokerage/accounts", app.address))
        .bearer_auth(token)
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 501);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "not_implemented");
}

#[tokio::test]
async fn brokerage_account_activities_path_param_routes_correctly() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("activities@example.com"),
        None,
        Duration::from_secs(600),
    );

    let res = app
        .client
        .get(format!(
            "{}/api/v1/sync/brokerage/accounts/abc-123/activities",
            app.address
        ))
        .bearer_auth(&token)
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 501);

    let res2 = app
        .client
        .get(format!(
            "{}/api/v1/sync/brokerage/accounts/abc-123/holdings",
            app.address
        ))
        .bearer_auth(token)
        .send()
        .await
        .expect("request");
    assert_eq!(res2.status(), 501);
}
