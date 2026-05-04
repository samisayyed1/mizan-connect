//! Mapping from SnapTrade-side failures to our [`AppError`].
//!
//! **Never** leak SnapTrade-specific text or status to the client —
//! `tracing::error!` logs the full upstream response body for operators,
//! while the API caller sees only the generic external messages defined
//! in the table below.
//!
//! | SnapTrade status | AppError | external message |
//! |------------------|----------|------------------|
//! | 401 (bad signature / clock skew / wrong key) | `Internal` | "broker integration error" |
//! | 403 | `Forbidden` | "broker access denied" |
//! | 404 | `NotFound` | "broker connection or account not found" |
//! | 429 | `TooManyRequests` | "upstream rate limit" |
//! | 5xx | `ServiceUnavailable` | "broker temporarily unavailable" |
//! | timeout / network | `ServiceUnavailable` | "broker request timed out" |
//! | parse-failure | `Internal` | "broker integration error" |

use reqwest::StatusCode;

use crate::error::AppError;

/// Errors raised by [`crate::snaptrade::client::SnaptradeClient`].
#[derive(Debug, thiserror::Error)]
pub enum SnaptradeError {
    #[error("snaptrade http error: {status}")]
    Status {
        status: StatusCode,
        body: String, // logged, never returned
    },
    #[error("transport error")]
    Transport(#[source] reqwest::Error),
    #[error("response decode error")]
    Decode(#[source] reqwest::Error),
    #[error("response parse error: {0}")]
    Parse(String),
}

impl From<SnaptradeError> for AppError {
    fn from(err: SnaptradeError) -> Self {
        match err {
            SnaptradeError::Status { status, body } => {
                // Log full upstream body for operators, never return it.
                tracing::error!(
                    snaptrade.status = status.as_u16(),
                    snaptrade.body = %truncate(&body, 512),
                    "SnapTrade API error",
                );
                match status.as_u16() {
                    401 => AppError::internal("broker integration error"),
                    403 => AppError::forbidden("broker access denied"),
                    404 => AppError::not_found("broker connection or account not found"),
                    429 => AppError::new(
                        crate::error::ErrorCode::TooManyRequests,
                        "upstream rate limit",
                    ),
                    s if (500..=599).contains(&s) => {
                        AppError::service_unavailable("broker temporarily unavailable")
                    }
                    _ => AppError::internal("broker integration error"),
                }
            }
            SnaptradeError::Transport(source) => {
                tracing::warn!(error = %source, "SnapTrade transport error");
                if source.is_timeout() {
                    AppError::service_unavailable("broker request timed out")
                } else {
                    AppError::service_unavailable("broker temporarily unavailable")
                }
            }
            SnaptradeError::Decode(source) => {
                tracing::error!(error = %source, "SnapTrade response decode failed");
                AppError::internal("broker integration error")
            }
            SnaptradeError::Parse(msg) => {
                tracing::error!(error = %msg, "SnapTrade response parse failed");
                AppError::internal("broker integration error")
            }
        }
    }
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // char_indices ensures we slice on a UTF-8 boundary.
        match s.char_indices().nth(max) {
            Some((idx, _)) => &s[..idx],
            None => s,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use crate::error::ErrorCode;

    #[test]
    fn status_401_maps_to_internal() {
        let err: AppError = SnaptradeError::Status {
            status: StatusCode::UNAUTHORIZED,
            body: "any internal detail".into(),
        }
        .into();
        assert_eq!(err.code(), ErrorCode::Internal);
        assert_eq!(err.message(), "broker integration error");
    }

    #[test]
    fn status_404_maps_to_not_found() {
        let err: AppError = SnaptradeError::Status {
            status: StatusCode::NOT_FOUND,
            body: String::new(),
        }
        .into();
        assert_eq!(err.code(), ErrorCode::NotFound);
    }

    #[test]
    fn status_429_maps_to_too_many() {
        let err: AppError = SnaptradeError::Status {
            status: StatusCode::TOO_MANY_REQUESTS,
            body: String::new(),
        }
        .into();
        assert_eq!(err.code(), ErrorCode::TooManyRequests);
    }

    #[test]
    fn status_503_maps_to_service_unavailable() {
        let err: AppError = SnaptradeError::Status {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: String::new(),
        }
        .into();
        assert_eq!(err.code(), ErrorCode::ServiceUnavailable);
    }
}
