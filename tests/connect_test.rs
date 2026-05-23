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
    assert_eq!(alias["supabaseUserId"], legacy["supabaseUserId"]);
    assert_eq!(alias["email"], "alias@example.com");
    assert_eq!(alias["displayName"], "Alias Tester");
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
// /api/v1/subscription/plans — Chunk 4: real plan catalog (Basic/Pro/Enterprise)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn subscription_plans_returns_catalog_with_auth() {
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

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("json");
    let plans = body["plans"].as_array().expect("plans array");
    let slugs: Vec<&str> = plans
        .iter()
        .map(|p| p["id"].as_str().unwrap_or(""))
        .collect();
    assert!(slugs.contains(&"basic"));
    assert!(slugs.contains(&"pro"));
    assert!(slugs.contains(&"enterprise"));
}

#[tokio::test]
async fn subscription_plans_returns_catalog_without_auth() {
    // The desktop hits this path anonymously too
    // (see fetch_subscription_plans_public in crates/connect).
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/api/v1/subscription/plans", app.address))
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 200, "no auth required for plans");
    let body: serde_json::Value = res.json().await.expect("json");
    assert!(body["plans"].as_array().expect("plans array").len() >= 3);
}
