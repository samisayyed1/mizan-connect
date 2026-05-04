//! Application-wide error type and HTTP response shape.
//!
//! All handlers return `Result<_, AppError>`. `AppError` decides the HTTP
//! status, the machine-readable code, and the user-visible message. Internal
//! details (DB errors, paths, etc.) are logged via `tracing` and **never**
//! returned to the client.

use std::borrow::Cow;
use std::fmt;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

use crate::middleware::request_id::RequestId;

/// Machine-readable error code surfaced to API clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    PayloadTooLarge,
    UnprocessableEntity,
    TooManyRequests,
    Internal,
    NotImplemented,
    ServiceUnavailable,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "not_found",
            Self::Conflict => "conflict",
            Self::PayloadTooLarge => "payload_too_large",
            Self::UnprocessableEntity => "unprocessable_entity",
            Self::TooManyRequests => "too_many_requests",
            Self::Internal => "internal_error",
            Self::NotImplemented => "not_implemented",
            Self::ServiceUnavailable => "service_unavailable",
        }
    }

    pub const fn status(self) -> StatusCode {
        match self {
            Self::BadRequest => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::UnprocessableEntity => StatusCode::UNPROCESSABLE_ENTITY,
            Self::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::NotImplemented => StatusCode::NOT_IMPLEMENTED,
            Self::ServiceUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}

/// Domain error returned by handlers. The caller picks a code and message;
/// any underlying source is preserved for tracing.
#[derive(Debug, thiserror::Error)]
pub struct AppError {
    code: ErrorCode,
    message: Cow<'static, str>,
    #[source]
    source: Option<Box<dyn std::error::Error + Send + Sync + 'static>>,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            code,
            message: message.into(),
            source: None,
        }
    }

    pub fn with_source<E>(mut self, source: E) -> Self
    where
        E: std::error::Error + Send + Sync + 'static,
    {
        self.source = Some(Box::new(source));
        self
    }

    pub fn bad_request(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::BadRequest, message)
    }

    pub fn unauthorized(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::Unauthorized, message)
    }

    pub fn forbidden(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::Forbidden, message)
    }

    pub fn not_found(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn conflict(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::Conflict, message)
    }

    pub fn unprocessable(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::UnprocessableEntity, message)
    }

    pub fn internal(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub fn not_implemented(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::NotImplemented, message)
    }

    pub fn service_unavailable(message: impl Into<Cow<'static, str>>) -> Self {
        Self::new(ErrorCode::ServiceUnavailable, message)
    }

    pub fn code(&self) -> ErrorCode {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}

impl From<sqlx::Error> for AppError {
    fn from(err: sqlx::Error) -> Self {
        match &err {
            sqlx::Error::RowNotFound => AppError::not_found("resource not found"),
            sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
                AppError::conflict("resource already exists")
            }
            _ => AppError::internal("database error").with_source(err),
        }
    }
}

impl From<reqwest::Error> for AppError {
    fn from(err: reqwest::Error) -> Self {
        AppError::service_unavailable("upstream request failed").with_source(err)
    }
}

impl From<serde_json::Error> for AppError {
    fn from(err: serde_json::Error) -> Self {
        AppError::bad_request("invalid JSON").with_source(err)
    }
}

impl From<validator::ValidationErrors> for AppError {
    fn from(err: validator::ValidationErrors) -> Self {
        AppError::unprocessable(format!("validation failed: {err}"))
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorBodyInner<'a>,
}

#[derive(Serialize)]
struct ErrorBodyInner<'a> {
    code: &'a str,
    message: &'a str,
    request_id: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        // Pull request_id from the current tracing span (set by middleware).
        let request_id = RequestId::current().unwrap_or_default().to_string();

        // Log everything we're not telling the client.
        let status = self.code.status();
        if status.is_server_error() {
            tracing::error!(
                error.code = self.code.as_str(),
                error.message = %self.message,
                error.source = ?self.source,
                http.status = status.as_u16(),
                "request failed (server error)"
            );
        } else {
            tracing::warn!(
                error.code = self.code.as_str(),
                error.message = %self.message,
                http.status = status.as_u16(),
                "request failed (client error)"
            );
        }

        let body = ErrorBody {
            error: ErrorBodyInner {
                code: self.code.as_str(),
                message: &self.message,
                request_id,
            },
        };

        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn error_codes_map_to_statuses() {
        assert_eq!(ErrorCode::BadRequest.status(), StatusCode::BAD_REQUEST);
        assert_eq!(ErrorCode::Unauthorized.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            ErrorCode::TooManyRequests.status(),
            StatusCode::TOO_MANY_REQUESTS
        );
    }

    #[test]
    fn sqlx_row_not_found_maps_to_404() {
        let err: AppError = sqlx::Error::RowNotFound.into();
        assert_eq!(err.code(), ErrorCode::NotFound);
    }
}
