//! Liveness (`/health`) and readiness (`/ready`) endpoints.

use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;

use crate::state::AppState;

/// `/health` payload.
#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
    version: &'static str,
    build_time: &'static str,
    commit_sha: &'static str,
}

/// `/ready` payload — surfaces dependency state to operators.
#[derive(Debug, Serialize)]
struct ReadyResponse {
    status: &'static str,
    db: ComponentStatus,
    jwks: ComponentStatus,
}

#[derive(Debug, Serialize)]
struct ComponentStatus {
    healthy: bool,
    detail: Option<String>,
}

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/health", get(health))
        .route("/ready", get(ready))
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        version: crate::VERSION,
        build_time: crate::BUILD_TIME,
        commit_sha: crate::GIT_COMMIT,
    })
}

async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    // DB ping with a short timeout so a hung pool doesn't pin the request.
    let db_status = match tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(state.db()),
    )
    .await
    {
        Ok(Ok(_)) => ComponentStatus {
            healthy: true,
            detail: None,
        },
        Ok(Err(err)) => ComponentStatus {
            healthy: false,
            detail: Some(format!("query: {err}")),
        },
        Err(_) => ComponentStatus {
            healthy: false,
            detail: Some("timed out after 2s".into()),
        },
    };

    let snapshot = state.jwks().snapshot();
    let jwks_status = if snapshot.key_count > 0 && !snapshot.stale {
        ComponentStatus {
            healthy: true,
            detail: None,
        }
    } else {
        ComponentStatus {
            healthy: false,
            detail: snapshot.last_error.or_else(|| {
                Some(if snapshot.key_count == 0 {
                    "no keys cached".into()
                } else {
                    "cache stale".into()
                })
            }),
        }
    };

    let healthy = db_status.healthy && jwks_status.healthy;
    let body = ReadyResponse {
        status: if healthy { "ready" } else { "not_ready" },
        db: db_status,
        jwks: jwks_status,
    };

    let status = if healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (status, Json(body))
}
