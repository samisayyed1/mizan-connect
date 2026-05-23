//! `/v1/me` auth + happy-path tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
use std::time::Duration;

use common::TestApp;
use uuid::Uuid;

const VALID_SUB: &str = "11111111-1111-1111-1111-111111111111";

#[tokio::test]
async fn me_returns_401_without_token() {
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/v1/me", app.address))
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "unauthorized");
    assert!(
        body["error"]["request_id"].as_str().is_some(),
        "request_id present"
    );
}

#[tokio::test]
async fn me_returns_401_with_malformed_authorization() {
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/v1/me", app.address))
        .header("authorization", "NotBearer abc")
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn me_returns_401_with_garbage_token() {
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/v1/me", app.address))
        .bearer_auth("definitely.not.a.jwt")
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn me_returns_401_with_expired_token() {
    let app = TestApp::spawn().await;
    let token = app.mint_expired_jwt(VALID_SUB);
    let res = app
        .client
        .get(format!("{}/v1/me", app.address))
        .bearer_auth(token)
        .send()
        .await
        .expect("request");
    assert_eq!(res.status(), 401);
}

#[tokio::test]
async fn me_returns_user_with_valid_token() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("alice@example.com"),
        Some("Alice"),
        Duration::from_secs(600),
    );

    let res = app
        .client
        .get(format!("{}/v1/me", app.address))
        .bearer_auth(token)
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 200, "expected 200, got {}", res.status());
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["email"], "alice@example.com");
    assert_eq!(body["displayName"], "Alice");
    assert_eq!(body["supabaseUserId"], VALID_SUB);
    assert!(Uuid::parse_str(body["id"].as_str().expect("id str")).is_ok());
}

#[tokio::test]
async fn second_call_with_same_subject_returns_same_local_id() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("alice@example.com"),
        None,
        Duration::from_secs(600),
    );

    let first: serde_json::Value = app
        .client
        .get(format!("{}/v1/me", app.address))
        .bearer_auth(&token)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");
    let second: serde_json::Value = app
        .client
        .get(format!("{}/v1/me", app.address))
        .bearer_auth(&token)
        .send()
        .await
        .expect("request")
        .json()
        .await
        .expect("json");

    assert_eq!(first["id"], second["id"], "upsert is idempotent");
}

#[tokio::test]
async fn me_returns_401_when_token_missing_email() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(VALID_SUB, None, None, Duration::from_secs(600));

    let res = app
        .client
        .get(format!("{}/v1/me", app.address))
        .bearer_auth(token)
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 401);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "unauthorized");
}

#[tokio::test]
async fn patch_me_updates_display_name() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("alice@example.com"),
        Some("Alice"),
        Duration::from_secs(600),
    );

    let res = app
        .client
        .patch(format!("{}/v1/me", app.address))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "display_name": "Alice II" }))
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 200);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["displayName"], "Alice II");
}

#[tokio::test]
async fn patch_me_rejects_too_long_name() {
    let app = TestApp::spawn().await;
    let token = app.mint_jwt(
        VALID_SUB,
        Some("alice@example.com"),
        Some("Alice"),
        Duration::from_secs(600),
    );

    let too_long = "a".repeat(200);
    let res = app
        .client
        .patch(format!("{}/v1/me", app.address))
        .bearer_auth(&token)
        .json(&serde_json::json!({ "display_name": too_long }))
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 422);
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["error"]["code"], "unprocessable_entity");
}
