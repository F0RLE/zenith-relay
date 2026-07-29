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
        if quota.limit_reached {
            return Self::Exhausted;
        }
        if quota
            .updated_at_ms
            .is_some_and(|updated_at| now_ms.saturating_sub(updated_at) > stale_after_ms)
        {
            return Self::Stale;
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
}
