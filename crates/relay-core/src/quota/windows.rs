use serde::{Deserialize, Serialize};
use std::fmt;

const DEFAULT_FULL_THRESHOLD_BASIS_POINTS: u16 = 9_950;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaWindowKind {
    Primary,
    Secondary,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum ResetTime {
    AbsoluteUnixSeconds(u64),
    AbsoluteUnixMilliseconds(u64),
    RelativeSeconds(u64),
}

impl ResetTime {
    pub fn normalize_ms(self, observed_at_ms: u64) -> u64 {
        match self {
            Self::AbsoluteUnixSeconds(seconds) => seconds.saturating_mul(1_000),
            Self::AbsoluteUnixMilliseconds(milliseconds) => milliseconds,
            Self::RelativeSeconds(seconds) => {
                observed_at_ms.saturating_add(seconds.saturating_mul(1_000))
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindowInput {
    pub kind: QuotaWindowKind,
    pub available_percent: Option<f64>,
    pub explicitly_full: Option<bool>,
    pub reset: Option<ResetTime>,
    pub window_minutes: Option<u32>,
    pub provider_cycle_id: Option<String>,
    pub observed_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindow {
    pub kind: QuotaWindowKind,
    #[serde(default)]
    pub provider_cycle_id: Option<String>,
    #[serde(default)]
    pub window_start_ms: Option<u64>,
    pub available_basis_points: Option<u16>,
    pub explicitly_full: Option<bool>,
    pub reset_at_ms: Option<u64>,
    pub window_minutes: Option<u32>,
    pub observed_at_ms: u64,
    pub full_transition_fingerprint: Option<String>,
    #[serde(default)]
    pub exhaustion_transition_fingerprint: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SupplementalQuotaWindow {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<crate::DefaultServiceTier>,
    pub window: QuotaWindow,
}

mod normalize;
mod snapshot;
mod subscription;

pub use snapshot::{QuotaErrorState, QuotaSnapshot};
pub(crate) use subscription::normalize_subscription_plan;
pub use subscription::{Subscription, SubscriptionInput, SubscriptionStatus};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaTransition {
    pub window_kind: QuotaWindowKind,
    pub fingerprint: String,
    pub transitioned_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QuotaNormalizationError {
    InvalidPercentage,
    MismatchedWindowKind,
    InvalidSupplementalWindow,
}

impl fmt::Display for QuotaNormalizationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPercentage => {
                formatter.write_str("quota percentage must be between 0 and 100")
            }
            Self::MismatchedWindowKind => {
                formatter.write_str("quota window kind does not match its slot")
            }
            Self::InvalidSupplementalWindow => {
                formatter.write_str("supplemental quota window is invalid")
            }
        }
    }
}

impl std::error::Error for QuotaNormalizationError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(kind: QuotaWindowKind, percent: f64, observed_at_ms: u64) -> QuotaWindowInput {
        QuotaWindowInput {
            kind,
            available_percent: Some(percent),
            explicitly_full: None,
            reset: Some(ResetTime::RelativeSeconds(60)),
            window_minutes: Some(300),
            provider_cycle_id: None,
            observed_at_ms,
        }
    }

    #[test]
    fn full_transition_and_fingerprint_survive_restart() {
        let previous =
            QuotaWindow::normalize(input(QuotaWindowKind::Primary, 40.0, 1_000), None).unwrap();
        assert_eq!(previous.reset_at_ms, Some(61_000));
        let full = QuotaWindow::normalize(
            input(QuotaWindowKind::Primary, 99.5, 2_000),
            Some(&previous),
        )
        .unwrap();
        let transition = full.full_transition_from(Some(&previous)).unwrap();
        assert_eq!(transition.window_kind, QuotaWindowKind::Primary);

        let serialized = serde_json::to_string(&full).unwrap();
        let restored: QuotaWindow = serde_json::from_str(&serialized).unwrap();
        let still_full = QuotaWindow::normalize(
            input(QuotaWindowKind::Primary, 100.0, 3_000),
            Some(&restored),
        )
        .unwrap();
        assert_eq!(
            still_full.full_transition_fingerprint,
            full.full_transition_fingerprint
        );
        assert!(still_full.full_transition_from(Some(&restored)).is_none());

        let used = QuotaWindow::normalize(
            input(QuotaWindowKind::Primary, 20.0, 4_000),
            Some(&still_full),
        )
        .unwrap();
        let next_full =
            QuotaWindow::normalize(input(QuotaWindowKind::Primary, 100.0, 5_000), Some(&used))
                .unwrap();
        assert_ne!(
            next_full.full_transition_fingerprint,
            full.full_transition_fingerprint
        );
    }

    #[test]
    fn exhaustion_transition_is_emitted_once_and_survives_refreshes() {
        let available =
            QuotaWindow::normalize(input(QuotaWindowKind::Secondary, 25.0, 1_000), None).unwrap();
        let exhausted = QuotaWindow::normalize(
            input(QuotaWindowKind::Secondary, 0.0, 2_000),
            Some(&available),
        )
        .unwrap();
        let transition = exhausted
            .exhaustion_transition_from(Some(&available))
            .unwrap();
        assert_eq!(transition.window_kind, QuotaWindowKind::Secondary);
        let repeated = QuotaWindow::normalize(
            input(QuotaWindowKind::Secondary, 0.0, 3_000),
            Some(&exhausted),
        )
        .unwrap();
        assert_eq!(
            repeated.exhaustion_transition_fingerprint,
            exhausted.exhaustion_transition_fingerprint
        );
        assert!(repeated
            .exhaustion_transition_from(Some(&exhausted))
            .is_none());
        let current = repeated.exhaustion_transition().unwrap();
        assert_eq!(current.fingerprint, transition.fingerprint);
        assert_eq!(current.transitioned_at_ms, repeated.observed_at_ms);
    }

    #[test]
    fn absolute_and_relative_reset_times_normalize_to_epoch_milliseconds() {
        assert_eq!(
            ResetTime::AbsoluteUnixSeconds(20).normalize_ms(1_000),
            20_000
        );
        assert_eq!(
            ResetTime::AbsoluteUnixMilliseconds(20).normalize_ms(1_000),
            20
        );
        assert_eq!(ResetTime::RelativeSeconds(20).normalize_ms(1_000), 21_000);
    }

    #[test]
    fn free_plan_detection_is_case_insensitive_and_keeps_paid_plans_distinct() {
        let subscription = |plan: &str| Subscription {
            plan_type: Some(plan.to_string()),
            ..Subscription::default()
        };
        assert!(subscription("Free").is_free_plan());
        assert!(subscription("chatgpt_free_tier").is_free_plan());
        assert!(!subscription("plus").is_free_plan());
        assert!(!Subscription::default().is_free_plan());
    }

    #[test]
    fn subscription_normalizes_known_openai_plan_aliases() {
        for (input, expected) in [
            ("chatgptplusplan", "plus"),
            ("ChatGPT Pro Plan", "pro"),
            ("chatgpt_business_plan", "business"),
            ("team", "business"),
            ("chatgpt_free_tier", "free"),
        ] {
            assert_eq!(normalize_subscription_plan(input), expected);
        }
    }
}
