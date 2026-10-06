use crate::quota::QuotaSnapshot;
use serde::{Deserialize, Serialize};

pub const QUOTA_STALE_AFTER_MS: u64 = 20 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "remaining")]
pub enum CandidateQuota {
    #[default]
    Unknown,
    Available(u64),
    /// No positive provider quota window is available, but the provider has
    /// explicitly confirmed that its separate credit ledger can still serve.
    CreditFallback,
    Exhausted,
    Stale,
}

impl CandidateQuota {
    pub fn from_snapshot(quota: &QuotaSnapshot, now_ms: u64, stale_after_ms: u64) -> Self {
        // A proven limit is terminal unless fresh provider credits explicitly
        // say the account can spend beyond the exhausted rate-limit window.
        let snapshot_is_fresh = quota
            .updated_at_ms
            .is_some_and(|updated_at| now_ms.saturating_sub(updated_at) <= stale_after_ms);
        if quota.limit_reached && !(snapshot_is_fresh && quota.has_usable_provider_credits()) {
            return Self::Exhausted;
        }
        if quota
            .updated_at_ms
            .is_some_and(|updated_at| now_ms.saturating_sub(updated_at) > stale_after_ms)
        {
            return Self::Stale;
        }
        // Keep percentage quota separate from provider credits. A credit-only
        // account remains schedulable, but must not look like it has one basis
        // point of actual quota when automatic routing ranks candidates.
        let window_remaining = quota
            .primary
            .iter()
            .chain(quota.secondary.iter())
            .filter_map(|window| window.available_basis_points)
            .map(u64::from)
            .min();
        match window_remaining {
            Some(0) if quota.has_usable_provider_credits() => Self::CreditFallback,
            Some(0) => Self::Exhausted,
            Some(remaining) => Self::Available(remaining),
            None if quota.has_usable_provider_credits() => Self::CreditFallback,
            None => Self::Unknown,
        }
    }

    pub(crate) fn is_eligible(self) -> bool {
        matches!(
            self,
            Self::Unknown | Self::Available(1..) | Self::CreditFallback
        )
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
    fn stale_provider_credit_observation_cannot_override_a_proven_limit() {
        let quota = QuotaSnapshot {
            limit_reached: true,
            provider_credits_available: true,
            available_credits_micro_units: Some(500_000_000),
            updated_at_ms: Some(1),
            ..Default::default()
        };

        assert_eq!(
            CandidateQuota::from_snapshot(&quota, 10_000, 1),
            CandidateQuota::Exhausted
        );
    }

    #[test]
    fn fresh_provider_credits_keep_zero_window_account_schedulable_without_inventing_quota() {
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
            CandidateQuota::CreditFallback
        );
    }

    #[test]
    fn fresh_provider_credits_without_a_window_use_the_credit_fallback_state() {
        let quota = QuotaSnapshot {
            provider_credits_available: true,
            available_credits_micro_units: Some(250_000_000),
            updated_at_ms: Some(1_000),
            ..Default::default()
        };

        assert_eq!(
            CandidateQuota::from_snapshot(&quota, 1_001, QUOTA_STALE_AFTER_MS),
            CandidateQuota::CreditFallback
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
