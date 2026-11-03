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
        let fresh_credits = quota.has_fresh_provider_credits(now_ms, stale_after_ms);
        if quota.limit_reached && !fresh_credits {
            return Self::Exhausted;
        }
        if quota.quota_observed_at_ms().is_some()
            && !crate::quota::observation_is_fresh(
                quota.quota_observed_at_ms(),
                now_ms,
                stale_after_ms,
            )
        {
            return if fresh_credits {
                Self::CreditFallback
            } else {
                Self::Stale
            };
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
            Some(0) if fresh_credits => Self::CreditFallback,
            Some(0) => Self::Exhausted,
            Some(remaining) => Self::Available(remaining),
            None if fresh_credits => Self::CreditFallback,
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
    fn unavailable_provider_ledger_cannot_rank_as_spendable_capacity() {
        for available in [false, true] {
            for unlimited in [false, true] {
                let snapshot = QuotaSnapshot {
                    provider_credits_available: available,
                    provider_credits_unlimited: unlimited,
                    available_credits_micro_units: Some(100_000_000),
                    provider_credits_observed_at_ms: Some(100),
                    updated_at_ms: Some(100),
                    ..Default::default()
                };
                let state =
                    crate::CandidateQuotaState::from_snapshot(&snapshot, 100, QUOTA_STALE_AFTER_MS);
                assert_eq!(
                    state.provider_credits_micro_units,
                    available.then_some(100_000_000)
                );
                assert_eq!(state.provider_credits_unlimited, available && unlimited);
                assert_eq!(
                    state.quota,
                    if available {
                        CandidateQuota::CreditFallback
                    } else {
                        CandidateQuota::Unknown
                    }
                );
                assert_eq!(snapshot.available_credits_micro_units, Some(100_000_000));
            }
        }
    }

    #[test]
    fn undated_or_future_credits_cannot_supply_fresh_capacity() {
        for updated_at_ms in [None, Some(2_000)] {
            let quota = QuotaSnapshot {
                provider_credits_available: true,
                available_credits_micro_units: Some(500_000_000),
                provider_credits_observed_at_ms: updated_at_ms,
                updated_at_ms,
                ..Default::default()
            };
            assert_ne!(
                CandidateQuota::from_snapshot(&quota, 1_000, QUOTA_STALE_AFTER_MS),
                CandidateQuota::CreditFallback
            );
            let exhausted = QuotaSnapshot {
                limit_reached: true,
                ..quota
            };
            assert_eq!(
                CandidateQuota::from_snapshot(&exhausted, 1_000, QUOTA_STALE_AFTER_MS),
                CandidateQuota::Exhausted
            );
        }
    }

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
            provider_credits_observed_at_ms: Some(1),
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
            provider_credits_observed_at_ms: Some(1_000),
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
            provider_credits_observed_at_ms: Some(1_000),
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
            provider_credits_observed_at_ms: Some(1_000),
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

    #[test]
    fn fresh_primary_does_not_rejuvenate_an_old_secondary_window() {
        let window = |kind, observed_at_ms| crate::quota::QuotaWindow {
            kind,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(4_200),
            explicitly_full: None,
            reset_at_ms: None,
            window_minutes: None,
            observed_at_ms,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        };
        let mut quota = QuotaSnapshot {
            primary: Some(window(crate::quota::QuotaWindowKind::Primary, 1_000)),
            secondary: Some(window(crate::quota::QuotaWindowKind::Secondary, 1)),
            updated_at_ms: Some(1_000),
            ..Default::default()
        };
        assert_eq!(
            CandidateQuota::from_snapshot(&quota, 1_000, 10),
            CandidateQuota::Stale
        );
        quota.provider_credits_available = true;
        quota.provider_credits_observed_at_ms = Some(1_000);
        assert_eq!(
            CandidateQuota::from_snapshot(&quota, 1_000, 10),
            CandidateQuota::CreditFallback
        );
    }
}
