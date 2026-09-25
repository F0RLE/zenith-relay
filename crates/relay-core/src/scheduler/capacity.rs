use crate::quota::QuotaSnapshot;
use serde::{Deserialize, Serialize};

pub const QUOTA_STALE_AFTER_MS: u64 = 20 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "remaining")]
pub enum CandidateQuota {
    #[default]
    Unknown,
    Available(u64),
    Exhausted,
    Stale,
}

impl CandidateQuota {
    pub fn from_snapshot(quota: &QuotaSnapshot, now_ms: u64, stale_after_ms: u64) -> Self {
        // A proven limit is terminal unless fresh provider credits explicitly
        // say the account can spend beyond the exhausted rate-limit window.
        if quota.limit_reached && !quota.has_usable_provider_credits() {
            return Self::Exhausted;
        }
        if quota
            .updated_at_ms
            .is_some_and(|updated_at| now_ms.saturating_sub(updated_at) > stale_after_ms)
        {
            return Self::Stale;
        }
        // Keep the reported percentage while its window still has room. A
        // separate provider credit balance must not reduce a 42% window to
        // one basis point: the protected-account reserve is measured in those
        // same basis points. Credits only keep an exhausted/unknown window
        // eligible, with the lowest known preference.
        let window_remaining = quota
            .primary
            .iter()
            .chain(quota.secondary.iter())
            .filter_map(|window| window.available_basis_points)
            .map(u64::from)
            .min();
        if quota.has_usable_provider_credits() {
            return Self::Available(
                window_remaining
                    .filter(|remaining| *remaining > 0)
                    .unwrap_or(1),
            );
        }
        match window_remaining {
            Some(0) => Self::Exhausted,
            Some(remaining) => Self::Available(remaining),
            None => Self::Unknown,
        }
    }

    pub(crate) fn is_eligible(self) -> bool {
        matches!(self, Self::Unknown | Self::Available(1..))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_provider_limit_stays_exhausted_even_when_the_snapshot_is_stale() {
        let quota = QuotaSnapshot {
            limit_reached: true,
            updated_at_ms: Some(1),
            ..Default::default()
        };

        assert_eq!(
            CandidateQuota::from_snapshot(&quota, 10_000, 1),
            CandidateQuota::Exhausted
        );
    }

    #[test]
    fn fresh_provider_credits_keep_zero_window_account_schedulable() {
        let quota = QuotaSnapshot {
            primary: Some(crate::quota::QuotaWindow {
                kind: crate::quota::QuotaWindowKind::Primary,
                provider_cycle_id: None,
                window_start_ms: None,
                available_basis_points: Some(0),
                explicitly_full: Some(true),
                reset_at_ms: Some(2_000),
                window_minutes: Some(300),
                observed_at_ms: 1_000,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
            }),
            limit_reached: true,
            provider_credits_available: true,
            updated_at_ms: Some(1_000),
            ..Default::default()
        };

        assert_eq!(
            CandidateQuota::from_snapshot(&quota, 1_001, QUOTA_STALE_AFTER_MS),
            CandidateQuota::Available(1)
        );
    }

    #[test]
    fn provider_credits_do_not_hide_remaining_percentage_quota() {
        let quota = QuotaSnapshot {
            primary: Some(crate::quota::QuotaWindow {
                kind: crate::quota::QuotaWindowKind::Primary,
                provider_cycle_id: None,
                window_start_ms: None,
                available_basis_points: Some(4_200),
                explicitly_full: None,
                reset_at_ms: Some(2_000),
                window_minutes: Some(43_200),
                observed_at_ms: 1_000,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
            }),
            provider_credits_available: true,
            available_credits_micro_units: Some(250_000_000),
            updated_at_ms: Some(1_000),
            ..Default::default()
        };

        assert_eq!(
            CandidateQuota::from_snapshot(&quota, 1_001, QUOTA_STALE_AFTER_MS),
            CandidateQuota::Available(4_200)
        );
    }

    #[test]
    fn stale_provider_credits_are_not_used_for_scheduling() {
        let quota = QuotaSnapshot {
            provider_credits_available: true,
            updated_at_ms: Some(1),
            ..Default::default()
        };

        assert_eq!(
            CandidateQuota::from_snapshot(&quota, 10_000, 1),
            CandidateQuota::Stale
        );
    }
}
