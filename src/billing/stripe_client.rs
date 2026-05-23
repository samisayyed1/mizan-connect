//! Minimal Stripe HTTP client.
//!
//! Hand-rolled over `reqwest` rather than pulling in `async-stripe`:
//!   - the surface we need is small (Customer create, Checkout Session create,
//!     Billing Portal Session create, Webhook signature verify);
//!   - `async-stripe` is heavy and has periodic version-skew with our pinned
//!     `sqlx`/`uuid`;
//!   - we already depend on `hmac` + `sha2` for SnapTrade state JWTs.
//!
//! Stripe's REST API takes `application/x-www-form-urlencoded` bodies with
//! bracketed keys for nested objects (`metadata[plan]=basic`). We build those
//! forms by hand — there are only a handful of fields.

use std::time::Duration;

use hmac::{Hmac, Mac};
use reqwest::Client;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sha2::Sha256;

const STRIPE_API_BASE: &str = "https://api.stripe.com";
const STRIPE_API_VERSION: &str = "2024-12-18.acacia";

/// HTTP client wrapping a Stripe secret key.
///
/// Cheap to clone — wraps an `Arc`'d `reqwest::Client`.
#[derive(Clone)]
pub struct StripeClient {
    http: Client,
    secret_key: SecretString,
}

impl std::fmt::Debug for StripeClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StripeClient")
            .field("base", &STRIPE_API_BASE)
            .field("secret_key", &"<redacted>")
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StripeError {
    #[error("stripe request failed: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("stripe returned HTTP {status}: {body}")]
    Api { status: u16, body: String },
    #[error("stripe webhook signature invalid: {0}")]
    Signature(&'static str),
}

impl StripeClient {
    pub fn new(secret_key: SecretString) -> Self {
        // Fall back to a default client if the builder somehow rejects our
        // options — the default still respects rustls + system DNS, just
        // without the 20s timeout cap. We never see this in practice.
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .user_agent(concat!("mizan-connect/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| Client::new());
        Self { http, secret_key }
    }

    /// Override the base URL for tests (point at a wiremock server).
    #[cfg(test)]
    pub fn with_base_for_tests(secret_key: SecretString, base: &str) -> StripeClientTest {
        StripeClientTest {
            inner: Self::new(secret_key),
            base: base.to_string(),
        }
    }

    fn base(&self) -> &str {
        STRIPE_API_BASE
    }

    async fn post_form<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        form: &[(&str, &str)],
    ) -> Result<T, StripeError> {
        let url = format!("{}{}", self.base(), path);
        post_form_impl(&self.http, &url, self.secret_key.expose_secret(), form).await
    }

    /// Create or look up a Stripe customer.
    ///
    /// Stripe doesn't expose "find by email" via REST cheaply — we create one
    /// per local user the first time they hit checkout and persist the id in
    /// `subscriptions.stripe_customer_id`. `metadata[user_id]=<uuid>` makes
    /// the customer back-pointable from the Dashboard.
    pub async fn create_customer(
        &self,
        email: &str,
        user_id: uuid::Uuid,
    ) -> Result<Customer, StripeError> {
        let user_id_str = user_id.to_string();
        let form = [
            ("email", email),
            ("metadata[user_id]", user_id_str.as_str()),
        ];
        self.post_form("/v1/customers", &form).await
    }

    /// Create a Checkout Session for a subscription.
    pub async fn create_checkout_session(
        &self,
        params: CheckoutSessionParams<'_>,
    ) -> Result<CheckoutSession, StripeError> {
        let user_id = params.client_reference_id.to_string();
        let form: Vec<(&str, &str)> = vec![
            ("mode", "subscription"),
            ("customer", params.customer_id),
            ("line_items[0][price]", params.price_id),
            ("line_items[0][quantity]", "1"),
            ("success_url", params.success_url),
            ("cancel_url", params.cancel_url),
            ("client_reference_id", user_id.as_str()),
            ("allow_promotion_codes", "true"),
        ];
        self.post_form("/v1/checkout/sessions", &form).await
    }

    /// Create a Billing Portal session for self-service plan management.
    pub async fn create_billing_portal_session(
        &self,
        customer_id: &str,
        return_url: &str,
    ) -> Result<PortalSession, StripeError> {
        let form = [("customer", customer_id), ("return_url", return_url)];
        self.post_form("/v1/billing_portal/sessions", &form).await
    }

    /// Verify a webhook signature. Stripe sends `t=<unix>,v1=<hex_hmac>`; we
    /// HMAC-SHA256 over `<t>.<raw_body>` with the webhook secret and
    /// constant-time compare against `v1`. Tolerance window: 5 minutes.
    ///
    /// Returns the unix timestamp on success so the caller can audit it.
    pub fn verify_webhook(
        whsec: &str,
        signature_header: &str,
        body: &[u8],
        now_unix: i64,
    ) -> Result<i64, StripeError> {
        let mut ts: Option<i64> = None;
        let mut sigs: Vec<&str> = Vec::new();
        for part in signature_header.split(',') {
            let mut kv = part.splitn(2, '=');
            match (kv.next(), kv.next()) {
                (Some("t"), Some(v)) => ts = v.trim().parse::<i64>().ok(),
                (Some("v1"), Some(v)) => sigs.push(v.trim()),
                _ => {}
            }
        }
        let Some(ts) = ts else {
            return Err(StripeError::Signature("missing timestamp"));
        };
        if sigs.is_empty() {
            return Err(StripeError::Signature("missing v1 signature"));
        }
        if (now_unix - ts).abs() > 300 {
            return Err(StripeError::Signature("timestamp outside tolerance"));
        }

        let mut mac = Hmac::<Sha256>::new_from_slice(whsec.as_bytes())
            .map_err(|_| StripeError::Signature("bad webhook secret"))?;
        mac.update(format!("{}.", ts).as_bytes());
        mac.update(body);
        let expected = mac.finalize().into_bytes();

        for sig_hex in sigs {
            if let Ok(bytes) = hex::decode(sig_hex) {
                if bool::from(subtle_eq(&expected, &bytes)) {
                    return Ok(ts);
                }
            }
        }
        Err(StripeError::Signature("no matching signature"))
    }
}

/// Constant-time slice comparison. (subtle crate isn't a dep; trivial to inline.)
fn subtle_eq(a: &[u8], b: &[u8]) -> subtle_choice::Choice {
    if a.len() != b.len() {
        return subtle_choice::Choice(0);
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    subtle_choice::Choice(((diff as u16).wrapping_sub(1) >> 8) as u8 & 1)
}

mod subtle_choice {
    pub struct Choice(pub u8);
    impl From<Choice> for bool {
        fn from(c: Choice) -> bool {
            c.0 == 1
        }
    }
}

async fn post_form_impl<T: serde::de::DeserializeOwned>(
    http: &Client,
    url: &str,
    secret: &str,
    form: &[(&str, &str)],
) -> Result<T, StripeError> {
    let resp = http
        .post(url)
        .basic_auth(secret, Some(""))
        .header("Stripe-Version", STRIPE_API_VERSION)
        .form(form)
        .send()
        .await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        return Err(StripeError::Api {
            status: status.as_u16(),
            body,
        });
    }
    serde_json::from_str::<T>(&body).map_err(|e| StripeError::Api {
        status: status.as_u16(),
        body: format!("decode error: {e} (body: {body})"),
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// Stripe response types (only the fields we read)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct Customer {
    pub id: String,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CheckoutSession {
    pub id: String,
    pub url: String,
}

#[derive(Debug, Deserialize)]
pub struct PortalSession {
    pub id: String,
    pub url: String,
}

/// Parameters for [`StripeClient::create_checkout_session`].
pub struct CheckoutSessionParams<'a> {
    pub customer_id: &'a str,
    pub price_id: &'a str,
    pub success_url: &'a str,
    pub cancel_url: &'a str,
    pub client_reference_id: uuid::Uuid,
}

#[cfg(test)]
pub struct StripeClientTest {
    pub inner: StripeClient,
    pub base: String,
}

#[cfg(test)]
impl StripeClientTest {
    pub async fn create_customer(
        &self,
        email: &str,
        user_id: uuid::Uuid,
    ) -> Result<Customer, StripeError> {
        let url = format!("{}/v1/customers", self.base);
        let user_id_str = user_id.to_string();
        let form = [
            ("email", email),
            ("metadata[user_id]", user_id_str.as_str()),
        ];
        post_form_impl(
            &self.inner.http,
            &url,
            self.inner.secret_key.expose_secret(),
            &form,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;

    fn sign(whsec: &str, ts: i64, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(whsec.as_bytes()).unwrap();
        mac.update(format!("{}.", ts).as_bytes());
        mac.update(body);
        let sig = mac.finalize().into_bytes();
        format!("t={},v1={}", ts, hex::encode(sig))
    }

    #[test]
    fn verifies_well_signed_webhook() {
        let body = br#"{"id":"evt_1","type":"customer.subscription.created"}"#;
        let header = sign("whsec_test", 1_700_000_000, body);
        let ts = StripeClient::verify_webhook("whsec_test", &header, body, 1_700_000_000).unwrap();
        assert_eq!(ts, 1_700_000_000);
    }

    #[test]
    fn rejects_tampered_body() {
        let body = br#"{"id":"evt_1"}"#;
        let header = sign("whsec_test", 1_700_000_000, body);
        let tampered = br#"{"id":"evt_2"}"#;
        assert!(
            StripeClient::verify_webhook("whsec_test", &header, tampered, 1_700_000_000).is_err()
        );
    }

    #[test]
    fn rejects_wrong_secret() {
        let body = br#"{}"#;
        let header = sign("whsec_real", 1_700_000_000, body);
        assert!(StripeClient::verify_webhook("whsec_wrong", &header, body, 1_700_000_000).is_err());
    }

    #[test]
    fn rejects_stale_timestamp() {
        let body = br#"{}"#;
        let header = sign("whsec_test", 1_700_000_000, body);
        // 10 minutes later — outside tolerance.
        assert!(StripeClient::verify_webhook("whsec_test", &header, body, 1_700_000_600).is_err());
    }

    #[test]
    fn rejects_missing_timestamp() {
        let header = "v1=deadbeef";
        assert!(StripeClient::verify_webhook("whsec", header, b"{}", 0).is_err());
    }
}
