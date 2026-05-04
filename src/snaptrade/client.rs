//! Typed wrapper around the SnapTrade REST API.
//!
//! Every outbound request is signed by [`crate::snaptrade::signing::sign`].
//! The base URL is configurable via `SNAPTRADE_API_BASE` so tests can
//! point at a `wiremock` server.
//!
//! Public methods on [`SnaptradeClient`] are typed; they return
//! [`crate::snaptrade::errors::SnaptradeError`] on failure, which the
//! handler layer converts to [`crate::error::AppError`] via `?`.

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use reqwest::{Method, StatusCode};
use secrecy::{ExposeSecret, SecretString};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use time::OffsetDateTime;

use crate::snaptrade::errors::SnaptradeError;
use crate::snaptrade::signing::{sign, SignInput};
use crate::snaptrade::types::{
    StAccount, StAuthorization, StLoginRequest, StLoginResponse, StRegisterUserRequest,
    StRegisterUserResponse,
};

const SIGNATURE_HEADER: &str = "Signature";
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// SnapTrade API client. Cheap to clone (wraps `Arc`).
#[derive(Clone)]
pub struct SnaptradeClient {
    inner: Arc<Inner>,
}

struct Inner {
    base_url: String,
    client_id: String,
    consumer_key: SecretString,
    http: reqwest::Client,
}

impl SnaptradeClient {
    pub fn new(
        base_url: impl Into<String>,
        client_id: impl Into<String>,
        consumer_key: SecretString,
    ) -> Result<Self, reqwest::Error> {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!("mizan-connect/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            inner: Arc::new(Inner {
                base_url: base_url.into().trim_end_matches('/').to_string(),
                client_id: client_id.into(),
                consumer_key,
                http,
            }),
        })
    }

    /// `POST /snapTrade/registerUser` — lazily called the first time a
    /// user opens the connect flow.
    pub(crate) async fn register_user(
        &self,
        user_id: &str,
    ) -> Result<StRegisterUserResponse, SnaptradeError> {
        let body = StRegisterUserRequest { user_id };
        self.send_signed(Method::POST, "/snapTrade/registerUser", &[], Some(&body))
            .await
    }

    /// `POST /snapTrade/login` — generates a one-time portal URL.
    /// `state_token` is appended to `custom_redirect` so the callback can
    /// bind the redirect to the user who initiated it.
    pub(crate) async fn login_user(
        &self,
        user_id: &str,
        user_secret: &SecretString,
        custom_redirect_with_state: &str,
    ) -> Result<StLoginResponse, SnaptradeError> {
        let body = StLoginRequest {
            broker: None,
            custom_redirect: custom_redirect_with_state,
            connection_type: "read",
            connection_portal_version: "v4",
            immediate_redirect: None,
        };
        self.send_signed(
            Method::POST,
            "/snapTrade/login",
            &[
                ("userId", user_id),
                ("userSecret", user_secret.expose_secret()),
            ],
            Some(&body),
        )
        .await
    }

    /// `GET /authorizations`
    pub(crate) async fn list_authorizations(
        &self,
        user_id: &str,
        user_secret: &SecretString,
    ) -> Result<Vec<StAuthorization>, SnaptradeError> {
        self.send_signed::<Value, Vec<StAuthorization>>(
            Method::GET,
            "/authorizations",
            &[
                ("userId", user_id),
                ("userSecret", user_secret.expose_secret()),
            ],
            None,
        )
        .await
    }

    /// `GET /accounts`
    pub(crate) async fn list_accounts(
        &self,
        user_id: &str,
        user_secret: &SecretString,
    ) -> Result<Vec<StAccount>, SnaptradeError> {
        self.send_signed::<Value, Vec<StAccount>>(
            Method::GET,
            "/accounts",
            &[
                ("userId", user_id),
                ("userSecret", user_secret.expose_secret()),
            ],
            None,
        )
        .await
    }

    /// `GET /accounts/{accountId}/holdings` — opaque pass-through.
    pub(crate) async fn account_holdings(
        &self,
        user_id: &str,
        user_secret: &SecretString,
        account_id: &str,
    ) -> Result<Value, SnaptradeError> {
        let path = format!("/accounts/{}/holdings", account_id);
        self.send_signed::<Value, Value>(
            Method::GET,
            &path,
            &[
                ("userId", user_id),
                ("userSecret", user_secret.expose_secret()),
            ],
            None,
        )
        .await
    }

    /// `GET /accounts/{accountId}/activities` — opaque pass-through with
    /// SnapTrade-native pagination params (offset, limit, dates).
    pub(crate) async fn account_activities(
        &self,
        user_id: &str,
        user_secret: &SecretString,
        account_id: &str,
        extra_params: &[(&str, &str)],
    ) -> Result<Value, SnaptradeError> {
        let path = format!("/accounts/{}/activities", account_id);
        let mut params: Vec<(&str, &str)> = vec![
            ("userId", user_id),
            ("userSecret", user_secret.expose_secret()),
        ];
        params.extend_from_slice(extra_params);
        self.send_signed::<Value, Value>(Method::GET, &path, &params, None)
            .await
    }

    /// `POST /authorizations/{id}/refresh`
    pub(crate) async fn refresh_authorization(
        &self,
        user_id: &str,
        user_secret: &SecretString,
        authorization_id: &str,
    ) -> Result<Value, SnaptradeError> {
        let path = format!("/authorizations/{}/refresh", authorization_id);
        self.send_signed::<Value, Value>(
            Method::POST,
            &path,
            &[
                ("userId", user_id),
                ("userSecret", user_secret.expose_secret()),
            ],
            None,
        )
        .await
    }

    /// `DELETE /authorizations/{id}` — removes a single broker connection
    /// from SnapTrade. Returns `Ok(())` whether the upstream deletes the
    /// row or 404's (idempotent).
    pub(crate) async fn remove_authorization(
        &self,
        user_id: &str,
        user_secret: &SecretString,
        authorization_id: &str,
    ) -> Result<(), SnaptradeError> {
        let path = format!("/authorizations/{}", authorization_id);
        let result: Result<Value, SnaptradeError> = self
            .send_signed::<Value, Value>(
                Method::DELETE,
                &path,
                &[
                    ("userId", user_id),
                    ("userSecret", user_secret.expose_secret()),
                ],
                None,
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(SnaptradeError::Status { status, .. }) if status == StatusCode::NOT_FOUND => {
                // Already removed upstream — treat as success.
                Ok(())
            }
            Err(other) => Err(other),
        }
    }

    /// Underlying signed-request executor. `params` are URL-encoded into
    /// the query string in caller-given order. `clientId` and `timestamp`
    /// are appended automatically.
    async fn send_signed<B, R>(
        &self,
        method: Method,
        path: &str,
        params: &[(&str, &str)],
        body: Option<&B>,
    ) -> Result<R, SnaptradeError>
    where
        B: Serialize,
        R: DeserializeOwned,
    {
        let timestamp = OffsetDateTime::now_utc().unix_timestamp().to_string();
        let mut all_params: Vec<(&str, &str)> = Vec::with_capacity(params.len() + 2);
        all_params.extend_from_slice(params);
        all_params.push(("clientId", &self.inner.client_id));
        all_params.push(("timestamp", &timestamp));

        let query = build_query_string(&all_params);

        // Serialize body once so the wire-bytes match the canonical-content
        // that gets signed (both routed through serde_json with sorted-key
        // BTreeMap behavior).
        let body_value: Option<Value> = match body {
            Some(b) => Some(
                serde_json::to_value(b)
                    .map_err(|e| SnaptradeError::Parse(format!("serialize body: {e}")))?,
            ),
            None => None,
        };
        let signature = sign(
            SignInput {
                path: &full_signing_path(path),
                query: &query,
                body: body_value.as_ref(),
            },
            &self.inner.consumer_key,
        );

        let url = format!("{}{}?{}", self.inner.base_url, path, query);
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        headers.insert(
            HeaderName::from_static("signature"),
            HeaderValue::from_str(&signature)
                .map_err(|e| SnaptradeError::Parse(format!("signature header: {e}")))?,
        );
        let _ = SIGNATURE_HEADER; // documentation anchor

        let mut req = self.inner.http.request(method, &url).headers(headers);
        if let Some(b) = body_value.as_ref() {
            req = req.json(b);
        }
        let response = req.send().await.map_err(SnaptradeError::Transport)?;
        let status = response.status();
        let bytes = response.bytes().await.map_err(SnaptradeError::Decode)?;

        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes).into_owned();
            return Err(SnaptradeError::Status { status, body });
        }
        if bytes.is_empty() {
            // Some endpoints (DELETE) reply with empty body; only valid
            // when R = Value or () — we coerce to JSON null.
            return serde_json::from_slice(b"null")
                .map_err(|e| SnaptradeError::Parse(format!("empty-body: {e}")));
        }
        serde_json::from_slice::<R>(&bytes)
            .map_err(|e| SnaptradeError::Parse(format!("decode response: {e}")))
    }
}

/// SnapTrade signs the path **including** the `/api/v1` prefix in the
/// canonical request, even though the `base_url` already contains it.
/// This helper reattaches `/api/v1` to the relative path used by the
/// client methods.
fn full_signing_path(relative: &str) -> String {
    if relative.starts_with("/api/") {
        relative.to_string()
    } else if relative.starts_with('/') {
        format!("/api/v1{}", relative)
    } else {
        format!("/api/v1/{}", relative)
    }
}

/// URL-encode `(key, value)` pairs in caller order, using `&` as a
/// separator. Matches Ruby `Faraday::Utils.build_query`'s canonical
/// behavior.
fn build_query_string(params: &[(&str, &str)]) -> String {
    use url::form_urlencoded::byte_serialize;
    let mut out = String::new();
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            out.push('&');
        }
        out.extend(byte_serialize(k.as_bytes()));
        out.push('=');
        out.extend(byte_serialize(v.as_bytes()));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn query_string_encodes_simple_pairs() {
        let q = build_query_string(&[("a", "1"), ("b", "two words")]);
        assert_eq!(q, "a=1&b=two+words");
    }

    #[test]
    fn signing_path_is_prefixed() {
        assert_eq!(
            full_signing_path("/snapTrade/login"),
            "/api/v1/snapTrade/login"
        );
        assert_eq!(full_signing_path("/api/v1/already"), "/api/v1/already");
    }
}
