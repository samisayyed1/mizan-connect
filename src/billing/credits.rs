//! Credit cost table for managed-AI requests.
//!
//! Mirrors the desktop's local cost table (manual §9). Pre-call estimate uses
//! the `RequestKind`; the post-call write reconciles with actual tokens via
//! [`credits_for_tokens`] so we charge the user what we actually spent
//! upstream, not the pre-estimate.

use serde::{Deserialize, Serialize};

/// What the user is asking the AI to do. Pre-call estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// Plain chat turn.
    Simple,
    /// Portfolio analysis / explanation.
    Analysis,
    /// CSV column mapping / classification.
    CsvMapping,
    /// Monthly summary report.
    MonthlyReport,
    /// Deep / multi-section report.
    DeepReport,
}

impl RequestKind {
    /// Pre-call credit estimate (manual §9). Always non-zero so a charge is
    /// reserved even when the request is short.
    pub const fn estimated_cost(self) -> i32 {
        match self {
            Self::Simple => 1,
            Self::Analysis => 5,
            Self::CsvMapping => 10,
            Self::MonthlyReport => 25,
            Self::DeepReport => 50,
        }
    }
}

/// Reconcile token usage → credit charge.
///
/// 1 credit per 1,000 prompt-equivalent tokens, with completion tokens
/// weighted ~3× (output is typically the dominant cost). Floor at 1 so even
/// a single-token reply costs something — predictability for the user.
pub fn credits_for_tokens(prompt_tokens: u32, completion_tokens: u32) -> i32 {
    let weighted = prompt_tokens as u64 + 3 * completion_tokens as u64;
    let credits = (weighted as f64 / 1000.0).ceil() as i32;
    credits.max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimated_cost_matches_manual() {
        assert_eq!(RequestKind::Simple.estimated_cost(), 1);
        assert_eq!(RequestKind::Analysis.estimated_cost(), 5);
        assert_eq!(RequestKind::CsvMapping.estimated_cost(), 10);
        assert_eq!(RequestKind::MonthlyReport.estimated_cost(), 25);
        assert_eq!(RequestKind::DeepReport.estimated_cost(), 50);
    }

    #[test]
    fn credit_floor_is_one() {
        assert_eq!(credits_for_tokens(0, 0), 1);
        assert_eq!(credits_for_tokens(10, 5), 1);
    }

    #[test]
    fn completion_weighted_more_than_prompt() {
        // 1000 prompt = 1 credit; 1000 completion = ~3 credits.
        assert_eq!(credits_for_tokens(1000, 0), 1);
        assert_eq!(credits_for_tokens(0, 1000), 3);
    }

    #[test]
    fn large_replies_cost_proportionally() {
        // 5k prompt + 10k completion ≈ 35k weighted → 35 credits.
        assert_eq!(credits_for_tokens(5_000, 10_000), 35);
    }
}
