//! Request-ID middleware.
//!
//! Reads `X-Request-Id` if the client supplied one, otherwise generates a
//! UUID v4. Stores the id on the tracing span for the request and echoes it
//! back on the response. Handlers (and `IntoResponse for AppError`) read the
//! id back via [`RequestId::current`].

use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{HeaderName, HeaderValue, Request, Response};
use futures::future::BoxFuture;
use tower::{Layer, Service};
use tracing::{field, Span};
use uuid::Uuid;

pub const HEADER_NAME: HeaderName = HeaderName::from_static("x-request-id");
pub const TRACING_FIELD: &str = "request_id";
const MAX_LEN: usize = 64;

/// Wrapper holding a request id pulled from the current tracing span.
#[derive(Debug, Clone, Default)]
pub struct RequestId(String);

impl RequestId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Read the id from the current tracing span, if set.
    pub fn current() -> Option<Self> {
        let mut out: Option<String> = None;
        Span::current().with_subscriber(|(id, sub)| {
            // Best-effort: the public tracing API does not let us pull a
            // field directly, so we encode it via a visitor.
            let _ = (id, sub); // silence unused warnings on no-op subscribers
        });
        // We instead stash the request id in a task-local set up by the
        // middleware. The Span approach above is intentionally a no-op kept
        // for documentation; see the task-local below.
        REQUEST_ID.try_with(|s| out = Some(s.clone())).ok();
        out.map(RequestId)
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

tokio::task_local! {
    static REQUEST_ID: String;
}

/// Tower [`Layer`] producing [`RequestIdService`].
#[derive(Clone, Default)]
pub struct RequestIdLayer;

impl<S> Layer<S> for RequestIdLayer {
    type Service = RequestIdService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        RequestIdService { inner }
    }
}

/// Wrapping service that injects a request id into the request, span, and
/// response, and exposes it via a tokio task-local for downstream handlers.
#[derive(Clone)]
pub struct RequestIdService<S> {
    inner: S,
}

impl<S> Service<Request<Body>> for RequestIdService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<Body>) -> Self::Future {
        let id = extract_or_generate(&req);

        // Inject on the request so handlers can pull it from headers too.
        // `id` is constrained by `is_safe_id_char` to ASCII alnum + `-_.`,
        // which is always a valid header value, so the fallback only fires
        // if a future change loosens the constraint.
        let header_value = HeaderValue::from_str(&id).unwrap_or_else(|_| {
            HeaderValue::from_str(&Uuid::new_v4().to_string())
                .unwrap_or(HeaderValue::from_static("invalid"))
        });
        req.headers_mut().insert(HEADER_NAME, header_value.clone());

        // Span field — populated by the trace layer; we only echo the value
        // through the task-local because tracing fields are write-only.
        Span::current().record(TRACING_FIELD, field::display(&id));

        // Clone first, then std::mem::replace, to satisfy the borrow checker.
        let cloned = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, cloned);
        let id_for_task = id.clone();
        Box::pin(async move {
            let fut = REQUEST_ID.scope(id_for_task, async move { inner.call(req).await });
            let mut response = fut.await?;
            response.headers_mut().insert(HEADER_NAME, header_value);
            Ok(response)
        })
    }
}

fn extract_or_generate<B>(req: &Request<B>) -> String {
    req.headers()
        .get(HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= MAX_LEN && s.chars().all(is_safe_id_char))
        .map(str::to_owned)
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

fn is_safe_id_char(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_id_with_unsafe_chars() {
        assert!(!is_safe_id_char('\n'));
        assert!(!is_safe_id_char(' '));
        assert!(is_safe_id_char('a'));
        assert!(is_safe_id_char('-'));
    }
}
