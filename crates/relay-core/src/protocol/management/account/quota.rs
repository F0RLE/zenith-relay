use crate::quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind};
use crate::ApiEquivalentSummary;
use serde::{Deserialize, Serialize};

const MIN_WEEKLY_WINDOW_MINUTES: u32 = 6 * 24 * 60;
const MAX_WEEKLY_WINDOW_MINUTES: u32 = 8 * 24 * 60;

/// API-price equivalent observed by Relay inside one provider quota window.
/// This is projection evidence, not an upstream balance or allowance.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindowUsage {
    pub kind: QuotaWindowKind,
    pub window_start_ms: u64,
    pub observed_at_ms: u64,
    pub window_minutes: u32,
    pub api_equivalent: ApiEquivalentSummary,
}

/// Chooses the provider's weekly window for a time-aligned value projection.
/// Short burst and monthly windows are deliberately excluded so the UI does
/// not present a different allowance as the weekly subscription value.
pub fn api_equivalent_projection_window(quota: &QuotaSnapshot) -> Option<&QuotaWindow> {
    quota
        .primary
        .iter()
        .chain(quota.secondary.iter())
        .filter(|window| {
            window.window_minutes.is_some_and(|minutes| {
                (MIN_WEEKLY_WINDOW_MINUTES..=MAX_WEEKLY_WINDOW_MINUTES).contains(&minutes)
            }) && window.window_start_ms.is_some()
                && window.available_basis_points.is_some()
                && window.observed_at_ms >= window.window_start_ms.unwrap_or_default()
        })
        .max_by_key(|window| window.window_minutes.unwrap_or_default())
}

#[cfg(test)]
mod quota_projection_tests {
    use super::*;

    fn window(kind: QuotaWindowKind, minutes: u32) -> QuotaWindow {
        QuotaWindow {
            kind,
            provider_cycle_id: None,
            window_start_ms: Some(1_000),
            available_basis_points: Some(5_000),
            explicitly_full: Some(false),
            reset_at_ms: Some(2_000),
            window_minutes: Some(minutes),
            observed_at_ms: 1_500,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }
    }

    #[test]
    fn projection_uses_the_long_window_instead_of_the_burst_window() {
        let quota = QuotaSnapshot {
            primary: Some(window(QuotaWindowKind::Primary, 300)),
            secondary: Some(window(QuotaWindowKind::Secondary, 10_080)),
            ..Default::default()
        };

        assert_eq!(
            api_equivalent_projection_window(&quota).map(|window| window.kind),
            Some(QuotaWindowKind::Secondary)
        );
    }

    #[test]
    fn projection_is_absent_when_only_a_burst_window_exists() {
        let quota = QuotaSnapshot {
            primary: Some(window(QuotaWindowKind::Primary, 300)),
            ..Default::default()
        };

        assert!(api_equivalent_projection_window(&quota).is_none());
    }

    #[test]
    fn projection_does_not_substitute_a_monthly_window_for_the_weekly_window() {
        let quota = QuotaSnapshot {
            secondary: Some(window(QuotaWindowKind::Secondary, 30 * 24 * 60)),
            ..Default::default()
        };

        assert!(api_equivalent_projection_window(&quota).is_none());
    }
}
