//! Security response headers.
//!
//! Returned as a [`SecurityHeaderLayers`] tuple consumed by the server
//! wiring. `Strict-Transport-Security` is added only in production.

use axum::http::header::{HeaderName, HeaderValue};
use tower_http::set_header::SetResponseHeaderLayer;

use crate::config::AppEnv;

const X_CONTENT_TYPE_OPTIONS: &str = "nosniff";
const X_FRAME_OPTIONS: &str = "DENY";
const REFERRER_POLICY: &str = "strict-origin-when-cross-origin";
const PERMISSIONS_POLICY: &str = "camera=(), geolocation=(), microphone=(), payment=()";
const HSTS: &str = "max-age=63072000; includeSubDomains; preload";
const X_PERMITTED_CROSS_DOMAIN_POLICIES: &str = "none";

type StaticHeaderLayer = SetResponseHeaderLayer<HeaderValue>;

/// Bundle of layers returned by [`layers`].
pub struct SecurityHeaderLayers {
    pub nosniff: StaticHeaderLayer,
    pub frame: StaticHeaderLayer,
    pub referrer: StaticHeaderLayer,
    pub permissions: StaticHeaderLayer,
    pub cross_domain: StaticHeaderLayer,
    pub hsts: Option<StaticHeaderLayer>,
}

/// Build the static set of security header layers.
pub fn layers(env: AppEnv) -> SecurityHeaderLayers {
    SecurityHeaderLayers {
        nosniff: SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static(X_CONTENT_TYPE_OPTIONS),
        ),
        frame: SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static(X_FRAME_OPTIONS),
        ),
        referrer: SetResponseHeaderLayer::overriding(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static(REFERRER_POLICY),
        ),
        permissions: SetResponseHeaderLayer::overriding(
            HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static(PERMISSIONS_POLICY),
        ),
        cross_domain: SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-permitted-cross-domain-policies"),
            HeaderValue::from_static(X_PERMITTED_CROSS_DOMAIN_POLICIES),
        ),
        hsts: env.is_production().then(|| {
            SetResponseHeaderLayer::overriding(
                HeaderName::from_static("strict-transport-security"),
                HeaderValue::from_static(HSTS),
            )
        }),
    }
}
