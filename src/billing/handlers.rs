//! HTTP handlers for billing endpoints.

use std::str::FromStr;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedUser;
use crate::error::AppError;
use crate::state::AppState;

use super::prices::{BillingInterval, CheckoutPlan};
use super::repository;
use super::stripe_client::CheckoutSessionParams;

// ─────────────────────────────────────────────────────────────────────────────
// POST /v1/billing/checkout-session
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CheckoutRequest {
    pub plan: String,
    pub interval: String,
}

#[derive(Debug, Serialize)]
pub struct CheckoutResponse {
    pub url: String,
}

pub async fn create_checkout_session(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(req): Json<CheckoutRequest>,
) -> Result<Json<CheckoutResponse>, AppError> {
    let billing = state
        .billing()
        .ok_or_else(|| AppError::not_implemented("billing is not configured on this server"))?;

    let plan = CheckoutPlan::from_str(&req.plan).map_err(AppError::bad_request)?;
    let interval = BillingInterval::from_str(&req.interval).map_err(AppError::bad_request)?;
    let price_id = billing
        .prices
        .lookup(plan, interval)
        .ok_or_else(|| AppError::bad_request("plan/interval not available"))?;

    // Ensure the user has a Stripe customer. Look up or create-and-persist.
    let customer_id = match repository::fetch_customer_id(state.db(), user.id).await? {
        Some(id) => id,
        None => {
            let customer = billing
                .stripe
                .create_customer(&user.email, user.id)
                .await
                .map_err(stripe_to_app_error)?;
            repository::upsert_customer_stub(state.db(), user.id, &customer.id).await?;
            customer.id
        }
    };

    // Stripe will redirect the browser back to these deep links — the
    // desktop's auth-callback page handles them by closing the tab and
    // letting the focus listener invalidate entitlements queries.
    let success_url = format!("{}?status=success", billing.return_url);
    let cancel_url = format!("{}?status=canceled", billing.return_url);

    let session = billing
        .stripe
        .create_checkout_session(CheckoutSessionParams {
            customer_id: &customer_id,
            price_id,
            success_url: &success_url,
            cancel_url: &cancel_url,
            client_reference_id: user.id,
        })
        .await
        .map_err(stripe_to_app_error)?;

    Ok(Json(CheckoutResponse { url: session.url }))
}

// ─────────────────────────────────────────────────────────────────────────────
// POST /v1/billing/portal
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PortalResponse {
    pub url: String,
}

pub async fn create_portal_session(
    State(state): State<AppState>,
    user: AuthenticatedUser,
) -> Result<Json<PortalResponse>, AppError> {
    let billing = state
        .billing()
        .ok_or_else(|| AppError::not_implemented("billing is not configured on this server"))?;

    let customer_id = repository::fetch_customer_id(state.db(), user.id)
        .await?
        .ok_or_else(|| AppError::not_found("no Stripe customer for this user"))?;

    let session = billing
        .stripe
        .create_billing_portal_session(&customer_id, &billing.return_url)
        .await
        .map_err(stripe_to_app_error)?;

    Ok(Json(PortalResponse { url: session.url }))
}

// ─────────────────────────────────────────────────────────────────────────────
// POST /v1/usage
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UsageRequest {
    pub metric: String,
    pub units: i32,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

pub async fn record_usage(
    State(state): State<AppState>,
    user: AuthenticatedUser,
    Json(req): Json<UsageRequest>,
) -> Result<StatusCode, AppError> {
    if !matches!(
        req.metric.as_str(),
        "ai_reply" | "broker_poll" | "csv_intel" | "market_refresh"
    ) {
        return Err(AppError::bad_request("unknown metric"));
    }
    if req.units < 0 {
        return Err(AppError::bad_request("units must be non-negative"));
    }

    let mut tx = state.db().begin().await?;
    repository::record_usage(
        &mut tx,
        user.id,
        &req.metric,
        req.units,
        0,
        req.model.as_deref(),
        req.kind.as_deref(),
    )
    .await?;
    tx.commit().await?;

    Ok(StatusCode::ACCEPTED)
}

// ─────────────────────────────────────────────────────────────────────────────
// GET /api/v1/subscription/plans (replaces the Chunk-3 stub)
// ─────────────────────────────────────────────────────────────────────────────

pub async fn list_plans() -> Result<Json<PlansResponse>, AppError> {
    // Hard-coded for now — the same shape the desktop frontend already
    // deserializes into its `SubscriptionPlan` type. Prices/limits mirror the
    // product manual; Stripe Price IDs are looked up at checkout time, not
    // shipped to clients (they'd be useless without the secret key anyway).
    Ok(Json(PlansResponse {
        plans: vec![
            PlanDto {
                id: "basic".into(),
                name: "Basic".into(),
                tagline: Some("Track your full wealth.".into()),
                description: "5 portfolios, full multi-currency, managed Mizan AI, device sync, cloud backup.".into(),
                pricing: PriceDto { monthly: 19.99, yearly: 199.0, yearly_per_month: Some(199.0 / 12.0) },
                limits: LimitsDto { household_size: 1, institution_connections: PlanLimitValueDto::Limited(0), devices: 5 },
                features: vec![
                    "5 portfolios".into(),
                    "All asset classes".into(),
                    "Managed Mizan AI (300 credits/mo)".into(),
                    "Device sync + cloud backup".into(),
                    "20 CSV imports/mo".into(),
                ],
                features_extended: None,
                is_available: true,
                is_coming_soon: false,
                badge: None,
                yearly_discount_percent: Some(17),
            },
            PlanDto {
                id: "pro".into(),
                name: "Pro".into(),
                tagline: Some("Serious investors, expats, multi-country families.".into()),
                description: "25 portfolios, unlimited holdings, 5 broker connections, advanced reports.".into(),
                pricing: PriceDto { monthly: 39.99, yearly: 399.0, yearly_per_month: Some(399.0 / 12.0) },
                limits: LimitsDto { household_size: 1, institution_connections: PlanLimitValueDto::Limited(5), devices: 10 },
                features: vec![
                    "Up to 25 portfolios".into(),
                    "Unlimited holdings".into(),
                    "5 broker connections".into(),
                    "Mizan AI (1500 credits/mo)".into(),
                    "Advanced reports + portfolio health".into(),
                ],
                features_extended: None,
                is_available: true,
                is_coming_soon: false,
                badge: Some("Most popular".into()),
                yearly_discount_percent: Some(17),
            },
            PlanDto {
                id: "enterprise".into(),
                name: "Enterprise".into(),
                tagline: Some("Advisors, accountants, family offices.".into()),
                description: "Unlimited everything, advisor/client dashboards, white-label reports.".into(),
                pricing: PriceDto { monthly: 249.0, yearly: 2490.0, yearly_per_month: Some(2490.0 / 12.0) },
                limits: LimitsDto { household_size: 50, institution_connections: PlanLimitValueDto::Unlimited("unlimited".into()), devices: 50 },
                features: vec![
                    "Unlimited portfolios + holdings".into(),
                    "Client dashboards + white-label".into(),
                    "Team roles".into(),
                    "Priority support".into(),
                ],
                features_extended: None,
                is_available: true,
                is_coming_soon: false,
                badge: None,
                yearly_discount_percent: Some(17),
            },
        ],
    }))
}

#[derive(Debug, Serialize)]
pub struct PlansResponse {
    pub plans: Vec<PlanDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanDto {
    pub id: String,
    pub name: String,
    pub tagline: Option<String>,
    pub description: String,
    pub pricing: PriceDto,
    pub limits: LimitsDto,
    pub features: Vec<String>,
    pub features_extended: Option<Vec<String>>,
    pub is_available: bool,
    pub is_coming_soon: bool,
    pub badge: Option<String>,
    pub yearly_discount_percent: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceDto {
    pub monthly: f64,
    pub yearly: f64,
    pub yearly_per_month: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitsDto {
    pub household_size: i32,
    pub institution_connections: PlanLimitValueDto,
    pub devices: i32,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum PlanLimitValueDto {
    Limited(i32),
    Unlimited(String),
}

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

fn stripe_to_app_error(err: super::stripe_client::StripeError) -> AppError {
    use super::stripe_client::StripeError;
    match err {
        StripeError::Transport(e) => {
            AppError::service_unavailable("upstream payment service unreachable").with_source(e)
        }
        StripeError::Api { status, body } => {
            tracing::error!(stripe_status = status, body = %body, "Stripe API error");
            AppError::service_unavailable("payment service returned an error")
        }
        StripeError::Signature(reason) => {
            tracing::warn!(reason = %reason, "Stripe webhook signature rejection");
            AppError::unauthorized("invalid webhook signature")
        }
    }
}
