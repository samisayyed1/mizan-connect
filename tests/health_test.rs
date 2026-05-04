//! Smoke tests for `/health` and `/ready`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
use common::TestApp;

#[tokio::test]
async fn health_returns_ok_with_metadata() {
    let app = TestApp::spawn().await;

    let res = app
        .client
        .get(format!("{}/health", app.address))
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 200, "health should return 200");
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["status"], "ok");
    assert!(body["version"].as_str().is_some(), "version present");
    assert!(body["build_time"].as_str().is_some(), "build_time present");
    assert!(body["commit_sha"].as_str().is_some(), "commit_sha present");
}

#[tokio::test]
async fn ready_reports_503_when_jwks_unreachable() {
    // The harness uses test.supabase.co which is unreachable, so JWKS is
    // empty and /ready should report not_ready.
    let app = TestApp::spawn().await;

    let res = app
        .client
        .get(format!("{}/ready", app.address))
        .send()
        .await
        .expect("request");

    assert_eq!(res.status(), 503, "ready should be 503 with empty JWKS");
    let body: serde_json::Value = res.json().await.expect("json");
    assert_eq!(body["status"], "not_ready");
    // DB component should still report healthy.
    assert_eq!(body["db"]["healthy"], true);
    assert_eq!(body["jwks"]["healthy"], false);
}

#[tokio::test]
async fn echoes_request_id_when_provided() {
    let app = TestApp::spawn().await;
    let req_id = "test-fixed-request-id-1";

    let res = app
        .client
        .get(format!("{}/health", app.address))
        .header("x-request-id", req_id)
        .send()
        .await
        .expect("request");

    let echoed = res
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    assert_eq!(echoed.as_deref(), Some(req_id));
}

#[tokio::test]
async fn generates_request_id_when_absent() {
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/health", app.address))
        .send()
        .await
        .expect("request");

    let id = res
        .headers()
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .expect("x-request-id present");
    assert!(!id.is_empty());
    assert!(uuid::Uuid::parse_str(id).is_ok(), "id is a uuid");
}

#[tokio::test]
async fn security_headers_present() {
    let app = TestApp::spawn().await;
    let res = app
        .client
        .get(format!("{}/health", app.address))
        .send()
        .await
        .expect("request");

    for header in [
        "x-content-type-options",
        "x-frame-options",
        "referrer-policy",
        "permissions-policy",
        "x-permitted-cross-domain-policies",
    ] {
        assert!(
            res.headers().contains_key(header),
            "expected header {header} to be set"
        );
    }
    // HSTS only in production.
    assert!(
        !res.headers().contains_key("strict-transport-security"),
        "HSTS must not be set outside production"
    );
}
