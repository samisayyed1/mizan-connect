//! Per-request timeout.
//!
//! Returns 504 Gateway Timeout when a handler exceeds the configured limit.

use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::BoxError;
use tower::timeout::error::Elapsed;
use tower_http::timeout::TimeoutLayer;

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Tower-http timeout layer with the project default. Returns
/// 504 Gateway Timeout on elapsed.
pub fn layer() -> TimeoutLayer {
    TimeoutLayer::with_status_code(StatusCode::GATEWAY_TIMEOUT, DEFAULT_TIMEOUT)
}

/// Map a `tower::timeout::error::Elapsed` (or any other `BoxError`) into an
/// HTTP response. Used by `HandleErrorLayer` if we ever need it.
#[allow(dead_code)]
pub async fn handle_error(err: BoxError) -> Response {
    if err.is::<Elapsed>() {
        (StatusCode::GATEWAY_TIMEOUT, "request timed out").into_response()
    } else {
        (StatusCode::INTERNAL_SERVER_ERROR, "internal error").into_response()
    }
}
