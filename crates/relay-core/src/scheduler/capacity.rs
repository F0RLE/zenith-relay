use crate::quota::QuotaSnapshot;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

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
        // Provider credits are an independent allowance. Match the upstream
        // account-pool behavior: fresh, explicitly positive or unlimited
        // credits keep a zero-window account schedulable, but at the lowest
        // known quota preference because they are not a percentage window.
        if quota.has_usable_provider_credits() {
            return Self::Available(1);
        }
        match quota
            .primary
            .iter()
            .chain(quota.secondary.iter())
            .filter_map(|window| window.available_basis_points)
            .map(u64::from)
            .min()
        {
            Some(0) => Self::Exhausted,
            Some(remaining) => Self::Available(remaining),
            None => Self::Unknown,
        }
    }

    pub(crate) fn is_eligible(self) -> bool {
        matches!(self, Self::Unknown | Self::Available(1..))
    }

    pub(crate) fn compare_preference(self, other: Self) -> Ordering {
        match (self, other) {
            (Self::Available(left), Self::Available(right)) => left.cmp(&right),
            (Self::Available(_), _) => Ordering::Greater,
            (_, Self::Available(_)) => Ordering::Less,
            _ => Ordering::Equal,
        }
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
