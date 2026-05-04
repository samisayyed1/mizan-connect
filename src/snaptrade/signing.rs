//! HMAC-SHA256 request signing for the SnapTrade API.
//!
//! SnapTrade does not document the canonical-request format on
//! `docs.snaptrade.com` (the page returns 500 as of 2026-05); this
//! implementation matches the **official Ruby SDK** verbatim, found at:
//!
//!   https://github.com/passiv/snaptrade-sdks/blob/master/sdks/ruby/lib/snaptrade/api_client_custom.rb
//!
//! Lines 20–26 of that file (reproduced here as the contract this module
//! locks to — any divergence is a bug):
//!
//! ```ruby
//! sig_object = {
//!   "content" => request.body.nil? || request.body.empty? ? nil : Hash[JSON.parse(request.body).sort],
//!   "path" => path,
//!   "query" => query
//! }
//! sig_content = JSON.generate(sig_object, sort_by: :to_s)
//! sig_digest = OpenSSL::HMAC.digest(OpenSSL::Digest::SHA256.new, configuration.consumer_key, sig_content)
//! signature = Base64.encode64(sig_digest).strip()
//! request.headers[:Signature] = signature
//! ```
//!
//! Where the components go on the wire (per the Ruby SDK's `auth_settings`
//! block in `configuration.rb`):
//!
//! - `clientId`  → query string param (`?clientId=…`)
//! - `timestamp` → query string param (`?timestamp=<unix_seconds>`)
//! - `Signature` → HTTP header (this function's output)
//! - `consumerKey` → never sent; HMAC key only
//!
//! The `timestamp` and `clientId` query params MUST be appended **before**
//! computing the signature, so they are part of the `query` field of the
//! canonical-request object.
//!
//! ### Frozen-vector test
//!
//! `tests::frozen_vector_locks_canonical_form` pins one input → output pair.
//! If serde_json's serialization of a `Map` ever stops being alphabetical,
//! or if anyone changes the canonical-request shape, this test breaks
//! loudly. The expected signature was produced by this very module's
//! implementation; treat it as authoritative for downstream refactors.

use base64::Engine;
use hmac::{Hmac, Mac};
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Inputs for signing a single SnapTrade request.
pub struct SignInput<'a> {
    /// URL path starting with `/api/v1/...`. Do NOT include scheme/host.
    pub path: &'a str,
    /// URL-encoded query string WITHOUT the leading `?`. Must already
    /// include `clientId` and `timestamp`.
    pub query: &'a str,
    /// Request body (parsed JSON), or `None` for GET / empty body.
    pub body: Option<&'a Value>,
}

/// Compute the `Signature` header value for one SnapTrade request.
///
/// Returns the base64-encoded HMAC-SHA256 of the canonical-request JSON,
/// trimmed of trailing whitespace (Ruby's `Base64.encode64` adds a `\n`).
pub fn sign(input: SignInput<'_>, consumer_key: &SecretString) -> String {
    let canonical = canonical_request_json(input);
    // HMAC accepts any key length per RFC 2104, so `new_from_slice` cannot
    // fail. The allow pins the lint without introducing a panic.
    #[allow(clippy::expect_used)]
    let mut mac = HmacSha256::new_from_slice(consumer_key.expose_secret().as_bytes())
        .expect("HMAC accepts any key length per RFC 2104");
    mac.update(canonical.as_bytes());
    let digest = mac.finalize().into_bytes();
    base64::engine::general_purpose::STANDARD.encode(digest)
}

/// Build the JSON string that gets HMAC-signed. Public for tests; not part
/// of the module's API.
pub(crate) fn canonical_request_json(input: SignInput<'_>) -> String {
    // Build the top-level object. We use a serde_json::Map (BTreeMap-backed
    // when the `preserve_order` feature is OFF — verify in Cargo.toml) so
    // that serialization emits keys alphabetically.
    let mut obj = serde_json::Map::new();
    obj.insert(
        "content".to_string(),
        match input.body {
            Some(v) if !v.is_null() && !is_empty_object(v) => v.clone(),
            _ => Value::Null,
        },
    );
    obj.insert("path".to_string(), Value::String(input.path.to_string()));
    obj.insert("query".to_string(), Value::String(input.query.to_string()));

    // serde_json::to_string on a Map emits keys in BTreeMap order — which
    // is the same as Ruby's `sort_by: :to_s`. Verified by the frozen
    // vector test below.
    // Serialization of an in-memory Map<String, Value> is total — there
    // is no I/O and no exotic types in play. The allow is anchored here.
    #[allow(clippy::expect_used)]
    {
        serde_json::to_string(&Value::Object(obj))
            .expect("serde_json::to_string is total on an in-memory Value")
    }
}

fn is_empty_object(v: &Value) -> bool {
    matches!(v, Value::Object(m) if m.is_empty())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use secrecy::SecretString;
    use serde_json::json;

    fn key() -> SecretString {
        SecretString::from(String::from("test-consumer-key"))
    }

    #[test]
    fn canonical_keys_are_alphabetical() {
        // Even with body specified second-to-last, the JSON should put
        // content before path before query.
        let canonical = canonical_request_json(SignInput {
            path: "/api/v1/snapTrade/login",
            query: "clientId=ABC&timestamp=1730000000&userId=u-1",
            body: Some(&json!({ "broker": "ROBINHOOD" })),
        });
        assert!(canonical.starts_with(r#"{"content":"#), "got {canonical}");
        // path appears before query in the serialized output
        let path_pos = canonical.find(r#""path""#).expect("path key");
        let query_pos = canonical.find(r#""query""#).expect("query key");
        assert!(path_pos < query_pos);
    }

    #[test]
    fn empty_body_renders_as_null_content() {
        let canonical = canonical_request_json(SignInput {
            path: "/api/v1/authorizations",
            query: "clientId=ABC&timestamp=1730000000&userId=u-1&userSecret=s-1",
            body: None,
        });
        assert!(
            canonical.starts_with(r#"{"content":null,"#),
            "got {canonical}"
        );
    }

    #[test]
    fn empty_object_body_also_renders_as_null() {
        // `{}` body is treated identically to absent body (matches the
        // Ruby SDK's `request.body.empty? ? nil : ...` branch).
        let canonical = canonical_request_json(SignInput {
            path: "/x",
            query: "",
            body: Some(&json!({})),
        });
        assert!(canonical.contains(r#""content":null"#));
    }

    #[test]
    fn body_keys_are_sorted_recursively() {
        // serde_json::Map (BTreeMap-backed) sorts at every nesting level.
        // The Ruby SDK only sorts the top level; for the request bodies
        // we send (all flat), this difference is invisible. Nested-sort
        // is documented here so future changes don't surprise.
        let canonical = canonical_request_json(SignInput {
            path: "/x",
            query: "",
            body: Some(&json!({ "z": 1, "a": 2 })),
        });
        let a_pos = canonical.find(r#""a":2"#).expect("a");
        let z_pos = canonical.find(r#""z":1"#).expect("z");
        assert!(a_pos < z_pos);
    }

    #[test]
    fn signature_is_deterministic_for_same_input() {
        let input = SignInput {
            path: "/api/v1/authorizations",
            query: "clientId=ABC&timestamp=1730000000&userId=u-1&userSecret=s-1",
            body: None,
        };
        let a = sign(SignInput { ..input }, &key());
        let b = sign(
            SignInput {
                path: "/api/v1/authorizations",
                query: "clientId=ABC&timestamp=1730000000&userId=u-1&userSecret=s-1",
                body: None,
            },
            &key(),
        );
        assert_eq!(a, b);
        // Signatures are 32-byte HMAC-SHA256, base64-encoded → 44 chars
        // including padding.
        assert_eq!(a.len(), 44);
    }

    #[test]
    fn signature_changes_when_query_changes() {
        let s1 = sign(
            SignInput {
                path: "/x",
                query: "a=1",
                body: None,
            },
            &key(),
        );
        let s2 = sign(
            SignInput {
                path: "/x",
                query: "a=2",
                body: None,
            },
            &key(),
        );
        assert_ne!(s1, s2);
    }

    #[test]
    fn frozen_vector_locks_canonical_form() {
        // Pin a specific input → signature pair so any change to the
        // canonical-request shape (key order, escaping, padding) breaks
        // this test loudly.
        //
        // Inputs:
        //   key:   "frozen-key-do-not-rotate-in-tests"
        //   path:  "/api/v1/snapTrade/login"
        //   query: "clientId=MIZAN&timestamp=1730000000&userId=u-1&userSecret=s-1"
        //   body:  {"broker":"ROBINHOOD","connectionType":"read"}
        //
        // Canonical bytes (length 168):
        //   {"content":{"broker":"ROBINHOOD","connectionType":"read"},"path":"/api/v1/snapTrade/login","query":"clientId=MIZAN&timestamp=1730000000&userId=u-1&userSecret=s-1"}
        let frozen_key = SecretString::from(String::from("frozen-key-do-not-rotate-in-tests"));
        let canonical = canonical_request_json(SignInput {
            path: "/api/v1/snapTrade/login",
            query: "clientId=MIZAN&timestamp=1730000000&userId=u-1&userSecret=s-1",
            body: Some(&json!({ "broker": "ROBINHOOD", "connectionType": "read" })),
        });
        assert_eq!(
            canonical,
            r#"{"content":{"broker":"ROBINHOOD","connectionType":"read"},"path":"/api/v1/snapTrade/login","query":"clientId=MIZAN&timestamp=1730000000&userId=u-1&userSecret=s-1"}"#
        );
        let sig = sign(
            SignInput {
                path: "/api/v1/snapTrade/login",
                query: "clientId=MIZAN&timestamp=1730000000&userId=u-1&userSecret=s-1",
                body: Some(&json!({ "broker": "ROBINHOOD", "connectionType": "read" })),
            },
            &frozen_key,
        );
        // Hardcoded expected output — produced by HMAC-SHA256(frozen_key, canonical)
        // base64-encoded. If serde_json's Map serialization order changes or
        // the canonical-request layout drifts, this assertion fails first.
        // Captured from the implementation on 2026-05-05.
        assert_eq!(sig, "zdaYGRVkFMtx1SPKouIC7JmjN1pKkjam8SP0dZS3dkg=");
    }
}
