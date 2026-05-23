//! Stripe Price ID lookup, sourced from env.
//!
//! Six product/interval combinations:
//!   - basic / monthly + yearly
//!   - pro / monthly + yearly
//!   - enterprise / monthly + yearly
//!
//! Each is configured separately so the operator can swap prices without a
//! redeploy. Missing env vars surface at config load time, not at first user
//! checkout.

use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Subscription plans the client can ask to subscribe to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckoutPlan {
    Basic,
    Pro,
    Enterprise,
}

impl FromStr for CheckoutPlan {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "basic" => Ok(Self::Basic),
            "pro" => Ok(Self::Pro),
            "enterprise" => Ok(Self::Enterprise),
            other => Err(format!("unknown plan: {other}")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BillingInterval {
    Monthly,
    Yearly,
}

impl FromStr for BillingInterval {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "monthly" | "month" => Ok(Self::Monthly),
            "yearly" | "year" | "annual" => Ok(Self::Yearly),
            other => Err(format!("unknown interval: {other}")),
        }
    }
}

/// Stripe Price ID configuration (six prices: 3 plans × 2 intervals).
#[derive(Debug, Clone)]
pub struct StripePrices {
    pub basic_monthly: String,
    pub basic_yearly: String,
    pub pro_monthly: String,
    pub pro_yearly: String,
    pub enterprise_monthly: String,
    pub enterprise_yearly: String,
}

impl StripePrices {
    /// Resolve to a single Price ID. Returns `None` only if the operator
    /// shipped an empty string for that slot — treated as "not for sale yet".
    pub fn lookup(&self, plan: CheckoutPlan, interval: BillingInterval) -> Option<&str> {
        let raw = match (plan, interval) {
            (CheckoutPlan::Basic, BillingInterval::Monthly) => &self.basic_monthly,
            (CheckoutPlan::Basic, BillingInterval::Yearly) => &self.basic_yearly,
            (CheckoutPlan::Pro, BillingInterval::Monthly) => &self.pro_monthly,
            (CheckoutPlan::Pro, BillingInterval::Yearly) => &self.pro_yearly,
            (CheckoutPlan::Enterprise, BillingInterval::Monthly) => &self.enterprise_monthly,
            (CheckoutPlan::Enterprise, BillingInterval::Yearly) => &self.enterprise_yearly,
        };
        if raw.is_empty() {
            None
        } else {
            Some(raw.as_str())
        }
    }

    /// Reverse lookup: a Stripe Price ID seen in a webhook → plan slug.
    /// Used by the subscription upsert to set the `tier` column when the
    /// Price's metadata is missing.
    pub fn plan_for(&self, price_id: &str) -> Option<CheckoutPlan> {
        if price_id == self.basic_monthly || price_id == self.basic_yearly {
            Some(CheckoutPlan::Basic)
        } else if price_id == self.pro_monthly || price_id == self.pro_yearly {
            Some(CheckoutPlan::Pro)
        } else if price_id == self.enterprise_monthly || price_id == self.enterprise_yearly {
            Some(CheckoutPlan::Enterprise)
        } else {
            None
        }
    }
}

impl CheckoutPlan {
    /// Stable lowercase slug stored in the `subscriptions.tier` ENUM.
    pub const fn slug(self) -> &'static str {
        match self {
            Self::Basic => "basic",
            Self::Pro => "pro",
            Self::Enterprise => "enterprise",
        }
    }
}
