//! Stripe billing + usage ledger (Chunk 4).
//!
//! The cloud half of the entitlements story documented in the desktop repo's
//! `docs/entitlements-cloud-contract.md`. The desktop's `crates/connect` calls
//! into the routes assembled here; the entitlements + AI-credits surface they
//! consume comes from [`crate::users::handlers::get_me`].
//!
//! Layout:
//!   - [`stripe_client`] — minimal HTTP wrapper for Stripe REST (Customer,
//!     Checkout, Portal) + webhook signature verification.
//!   - [`prices`] — env-driven Stripe Price ID lookup.
//!   - [`entitlements`] — tier→matrix table (mirrors the desktop's).
//!   - [`credits`] — managed-AI cost table.
//!   - [`repository`] — SQL for subscriptions + usage_ledger + stripe_events.
//!   - [`handlers`] — checkout/portal/usage HTTP handlers.
//!   - [`webhook`] — `/v1/stripe/webhook` receiver.

pub mod ai_proxy;
pub mod credits;
pub mod entitlements;
pub mod handlers;
pub mod prices;
pub mod reports;
pub mod reports_cron;
pub mod repository;
pub mod stripe_client;
pub mod webhook;

use std::sync::Arc;

use axum::routing::{get, post};
use axum::Router;
use secrecy::SecretString;

use crate::state::AppState;

pub use entitlements::{entitlements_for, AiCredits, Entitlements};
pub use prices::{BillingInterval, CheckoutPlan, StripePrices};
pub use stripe_client::StripeClient;

/// Everything billing handlers need from `AppState`. Optional so dev/test
/// builds without Stripe creds still boot.
#[derive(Clone)]
pub struct BillingContext {
    inner: Arc<BillingInner>,
}

pub struct BillingInner {
    pub stripe: StripeClient,
    pub prices: StripePrices,
    pub webhook_secret: SecretString,
    /// Where Stripe redirects the browser after Checkout / Portal. The
    /// desktop's auth-callback page handles this URL by closing the tab and
    /// letting the window-focus listener refresh entitlements.
    pub return_url: String,
    /// Optional OpenAI API key for the managed-AI proxy. When `None`, the
    /// `/v1/ai/chat` endpoint rejects with `service_unavailable`.
    pub openai_key: Option<SecretString>,
}

impl std::ops::Deref for BillingContext {
    type Target = BillingInner;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl BillingContext {
    pub fn new(
        stripe: StripeClient,
        prices: StripePrices,
        webhook_secret: SecretString,
        return_url: String,
        openai_key: Option<SecretString>,
    ) -> Self {
        Self {
            inner: Arc::new(BillingInner {
                stripe,
                prices,
                webhook_secret,
                return_url,
                openai_key,
            }),
        }
    }
}

/// `/v1/billing/*` + `/usage` + `/ai/chat` + Stripe webhook router. The webhook
/// route is signature-verified rather than JWT-authed.
pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/billing/checkout-session",
            post(handlers::create_checkout_session),
        )
        .route("/billing/portal", post(handlers::create_portal_session))
        .route("/usage", post(handlers::record_usage))
        .route("/ai/chat", post(ai_proxy::chat))
        // OpenAI-compatible alias: lets the desktop's `mizan` provider use a
        // stock rig-core OpenAI client by setting only the base URL (the
        // client appends `/chat/completions`). Same handler, same SSE
        // contract.
        .route("/chat/completions", post(ai_proxy::chat))
        // Monthly AI Wealth Report (M3.6). GET = list, POST = enqueue on-demand.
        .route(
            "/reports/monthly",
            get(reports::list_monthly_reports).post(reports::request_monthly_report),
        )
        .route("/stripe/webhook", post(webhook::receive_webhook))
}

/// Replacement for the Chunk-3 stub at `/api/v1/subscription/plans`.
pub fn plans_router() -> Router<AppState> {
    Router::new().route("/subscription/plans", get(handlers::list_plans))
}
