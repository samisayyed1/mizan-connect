//! Tier → entitlements matrix.
//!
//! Single source of truth on the cloud side; **must stay in lock-step with the
//! desktop's `crates/connect/src/entitlements.rs` table**. The /v1/me response
//! ships this matrix verbatim so the desktop's `Entitlements` struct deserializes
//! it directly.

use serde::{Deserialize, Serialize};

/// Sentinel meaning "no limit" for any numeric quota.
pub const UNLIMITED: i32 = -1;

/// Capabilities + quotas a subscription unlocks.
///
/// Field names + JSON shape mirror the desktop's `Entitlements` struct.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entitlements {
    /// Resolved plan slug this matrix is for (`free`/`basic`/`pro`/`enterprise`).
    pub plan: String,
    pub max_portfolios: i32,
    pub max_holdings: i32,
    pub max_asset_classes: i32,
    pub broker_sync: bool,
    pub max_broker_connections: i32,
    pub device_sync: bool,
    pub cloud_backup: bool,
    pub managed_ai: bool,
    pub ai_credits_monthly: i32,
    pub news_daily_limit: i32,
    pub market_refresh_daily_limit: i32,
    pub csv_imports_monthly: i32,
    pub advanced_reports: bool,
    pub advisor_mode: bool,
}

impl Default for Entitlements {
    fn default() -> Self {
        Self {
            plan: "free".to_string(),
            max_portfolios: 1,
            max_holdings: 20,
            max_asset_classes: 2,
            broker_sync: false,
            max_broker_connections: 0,
            device_sync: false,
            cloud_backup: false,
            managed_ai: false,
            ai_credits_monthly: 0,
            news_daily_limit: 3,
            market_refresh_daily_limit: 5,
            csv_imports_monthly: 1,
            advanced_reports: false,
            advisor_mode: false,
        }
    }
}

/// Derive entitlements from a stored subscription's `tier` slug + `status`.
///
/// Anything other than `active`/`trialing` collapses to Free. Legacy slugs
/// (`essentials`/`duo`/`plus`) map to the Pro matrix so existing rows behave
/// sensibly; new sign-ups land on `basic`/`pro`/`enterprise`.
pub fn entitlements_for(tier: Option<&str>, status: Option<&str>) -> Entitlements {
    let active = matches!(status, Some("active") | Some("trialing"));
    if !active {
        return Entitlements::default();
    }
    match tier.map(|t| t.to_ascii_lowercase()).as_deref() {
        None | Some("free") => Entitlements::default(),

        // Entry paid tier: device sync + managed AI + 300 credits, no broker.
        Some("basic") => Entitlements {
            plan: "basic".to_string(),
            max_portfolios: 5,
            max_holdings: 250,
            max_asset_classes: UNLIMITED,
            broker_sync: false,
            max_broker_connections: 0,
            device_sync: true,
            cloud_backup: true,
            managed_ai: true,
            ai_credits_monthly: 300,
            news_daily_limit: UNLIMITED,
            market_refresh_daily_limit: 50,
            csv_imports_monthly: 20,
            advanced_reports: false,
            advisor_mode: false,
        },

        // Enterprise / advisor tier.
        Some("enterprise") | Some("advisor") => Entitlements {
            plan: tier.unwrap_or("enterprise").to_ascii_lowercase(),
            max_portfolios: UNLIMITED,
            max_holdings: UNLIMITED,
            max_asset_classes: UNLIMITED,
            broker_sync: true,
            max_broker_connections: UNLIMITED,
            device_sync: true,
            cloud_backup: true,
            managed_ai: true,
            ai_credits_monthly: UNLIMITED,
            news_daily_limit: UNLIMITED,
            market_refresh_daily_limit: UNLIMITED,
            csv_imports_monthly: UNLIMITED,
            advanced_reports: true,
            advisor_mode: true,
        },

        // pro / essentials / duo / plus / any other active paid slug → Pro matrix.
        Some(other) => Entitlements {
            plan: other.to_string(),
            max_portfolios: 25,
            max_holdings: UNLIMITED,
            max_asset_classes: UNLIMITED,
            broker_sync: true,
            max_broker_connections: 5,
            device_sync: true,
            cloud_backup: true,
            managed_ai: true,
            ai_credits_monthly: 1500,
            news_daily_limit: UNLIMITED,
            market_refresh_daily_limit: UNLIMITED,
            csv_imports_monthly: UNLIMITED,
            advanced_reports: true,
            advisor_mode: false,
        },
    }
}

/// AI credit balance + reset window surfaced to clients on /v1/me.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AiCredits {
    pub monthly: i32,
    pub used: i32,
    pub resets_at: Option<time::OffsetDateTime>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inactive_falls_back_to_free() {
        assert_eq!(
            entitlements_for(Some("pro"), Some("canceled")),
            Entitlements::default()
        );
        assert_eq!(entitlements_for(Some("pro"), None), Entitlements::default());
        assert_eq!(entitlements_for(None, None), Entitlements::default());
    }

    #[test]
    fn basic_excludes_broker_sync() {
        let e = entitlements_for(Some("basic"), Some("active"));
        assert!(!e.broker_sync);
        assert!(e.device_sync);
        assert!(e.managed_ai);
        assert_eq!(e.max_portfolios, 5);
    }

    #[test]
    fn pro_legacy_slugs_unlock_broker_sync() {
        for slug in ["pro", "plus", "duo", "essentials"] {
            let e = entitlements_for(Some(slug), Some("active"));
            assert!(e.broker_sync, "{slug} should include broker sync");
            assert_eq!(e.ai_credits_monthly, 1500);
        }
    }

    #[test]
    fn enterprise_unlocks_advisor() {
        let e = entitlements_for(Some("enterprise"), Some("active"));
        assert!(e.advisor_mode);
        assert_eq!(e.max_portfolios, UNLIMITED);
    }

    #[test]
    fn trialing_counts_as_active() {
        assert!(entitlements_for(Some("pro"), Some("trialing")).broker_sync);
    }
}
