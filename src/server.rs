//! Axum router composition and server bootstrap.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, Method};
use axum::Router;
use tower::util::option_layer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::middleware::request_id::RequestIdLayer;
use crate::middleware::{security_headers, timeout};
use crate::state::AppState;

/// Maximum request body size accepted by the server.
const MAX_BODY_BYTES: usize = 1024 * 1024; // 1 MB

/// Build the fully-wired Axum router.
pub fn build_app(state: AppState) -> Router {
    let cors = build_cors(state.config());
    let governor = GovernorLayer {
        config: build_rate_limiter_config(state.config()),
    };
    let security_headers::SecurityHeaderLayers {
        nosniff,
        frame,
        referrer,
        permissions,
        cross_domain,
        hsts,
    } = security_headers::layers(state.config().app_env);

    let trace_layer = TraceLayer::new_for_http().make_span_with(|req: &axum::http::Request<_>| {
        let request_id = req
            .headers()
            .get(crate::middleware::request_id::HEADER_NAME)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        tracing::info_span!(
            "http_request",
            method = %req.method(),
            uri = %req.uri(),
            version = ?req.version(),
            request_id = %request_id,
        )
    });

    let v1 = Router::new()
        .merge(crate::users::router())
        .with_state(state.clone());

    Router::new()
        .merge(crate::health::router())
        .nest("/v1", v1)
        .with_state(state)
        // The order matters: outermost layer is added last.
        .layer(nosniff)
        .layer(frame)
        .layer(referrer)
        .layer(permissions)
        .layer(cross_domain)
        .layer(option_layer(hsts))
        .layer(cors)
        .layer(governor)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_BYTES))
        .layer(timeout::layer())
        .layer(RequestIdLayer)
        .layer(trace_layer)
        .layer(sentry_tower::NewSentryLayer::<axum::http::Request<_>>::new_from_top())
        .layer(sentry_tower::SentryHttpLayer::with_transaction())
}

fn build_cors(config: &Config) -> CorsLayer {
    let mut layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
            HeaderName::from_static("x-request-id"),
        ])
        .max_age(Duration::from_secs(60 * 10));

    for origin in &config.cors_allowed_origins {
        match origin.parse::<HeaderValue>() {
            Ok(value) => layer = layer.allow_origin(value),
            Err(err) => {
                tracing::warn!(origin = %origin, error = %err, "skipping malformed CORS origin");
            }
        }
    }
    layer
}

fn build_rate_limiter_config(
    config: &Config,
) -> Arc<
    tower_governor::governor::GovernorConfig<
        tower_governor::key_extractor::PeerIpKeyExtractor,
        governor::middleware::NoOpMiddleware,
    >,
> {
    let per_minute = config.rate_limit_per_minute.max(1);
    let per_second = (per_minute / 60).max(1);
    let burst = (per_minute / 4).max(1);

    // `finish()` only returns `None` for impossible values (zero burst /
    // zero period). Both are clamped to ≥ 1 above, so the unwrap is safe.
    #[allow(clippy::expect_used)]
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(u64::from(per_second))
        .burst_size(burst)
        .finish()
        .expect("rate limiter config is valid (per_second and burst clamped >= 1)");
    Arc::new(governor_conf)
}

/// Resolve the bind address from config.
pub fn bind_addr(config: &Config) -> SocketAddr {
    let host = config.app_host.parse().unwrap_or_else(|_| {
        tracing::warn!(host = %config.app_host, "APP_HOST is not a valid IP, defaulting to 0.0.0.0");
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED)
    });
    SocketAddr::from((host, config.app_port))
}
