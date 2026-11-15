use super::{QuotaSnapshot, Subscription};

const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const EXPIRY_GROUP_MS: u64 = 3 * 24 * 60 * 60 * 1_000;
const MAX_EXPIRY_WEIGHT: u32 = 8;

/// Fresh entitlement evidence used only to bias useful, independent work.
/// A deadline is neither available quota nor authorization to redeem a reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaSpendDeadline {
    pub expires_at_ms: u64,
    pub observed_at_ms: u64,
    pub valid_until_ms: u64,
    pub reset_credits: Option<QuotaSpendResets>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaSpendResets {
    pub count: u32,
    pub observed_at_ms: u64,
    pub valid_until_ms: u64,
}

impl QuotaSpendResets {
    pub fn from_snapshot(quota: &QuotaSnapshot, now_ms: u64, stale_after_ms: u64) -> Option<Self> {
        let observed_at_ms = quota.provider_credits_observed_at_ms?;
        let count = quota.reset_credits_available.filter(|count| *count > 0)?;
        (quota.error.is_none()
            && super::observation_is_fresh(Some(observed_at_ms), now_ms, stale_after_ms))
        .then_some(Self {
            count,
            observed_at_ms,
            valid_until_ms: observed_at_ms.saturating_add(stale_after_ms),
        })
    }
}

impl QuotaSpendDeadline {
    /// Three-day priority bands for fresh, unexpired entitlement evidence.
    pub fn expiry_group(self, now_ms: u64) -> Option<u64> {
        (self.observed_at_ms <= now_ms
            && self.valid_until_ms >= now_ms
            && self.expires_at_ms > now_ms)
            .then(|| (self.expires_at_ms - now_ms - 1) / EXPIRY_GROUP_MS)
    }

    pub fn from_subscription(
        subscription: &Subscription,
        now_ms: u64,
        stale_after_ms: u64,
    ) -> Option<Self> {
        Some(Self {
            expires_at_ms: subscription.active_deadline_ms(now_ms, stale_after_ms)?,
            observed_at_ms: subscription.updated_at_ms?,
            valid_until_ms: subscription.updated_at_ms?.saturating_add(stale_after_ms),
            reset_credits: None,
        })
    }

    /// A bounded weekly-headroom/time signal, not a demand or token forecast.
    /// Keep ordinary fairness when the allowance is not at risk of expiring.
    pub fn weight(self, remaining_basis_points: u64, now_ms: u64) -> u32 {
        if self.observed_at_ms > now_ms
            || self.valid_until_ms < now_ms
            || self.expires_at_ms <= now_ms
            || !(1..=10_000).contains(&remaining_basis_points)
        {
            return 1;
        }
        let remaining_ms = self.expires_at_ms - now_ms;
        let pressure = (u128::from(remaining_basis_points) * u128::from(WEEK_MS))
            .div_ceil(10_000 * u128::from(remaining_ms));
        u32::try_from(pressure.clamp(1, u128::from(MAX_EXPIRY_WEIGHT)))
            .expect("bounded expiry weight")
    }

    /// Reset inventory is a bounded scheduling estimate, never dispatch capacity.
    pub fn rotation_weight(self, remaining_basis_points: u64, now_ms: u64) -> u32 {
        if self.expiry_group(now_ms).is_none() || !(1..=10_000).contains(&remaining_basis_points) {
            return 1;
        }
        let resets = self
            .reset_credits
            .filter(|resets| resets.observed_at_ms <= now_ms && now_ms <= resets.valid_until_ms)
            .map_or(0, |resets| resets.count.min(MAX_EXPIRY_WEIGHT));
        let allowance = u128::from(remaining_basis_points) + u128::from(resets) * 10_000;
        let pressure = (allowance * u128::from(WEEK_MS))
            .div_ceil(10_000 * u128::from(self.expires_at_ms - now_ms));
        u32::try_from(pressure.clamp(1, u128::from(MAX_EXPIRY_WEIGHT)))
            .expect("bounded expiry weight")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::SubscriptionStatus;

    #[test]
    fn reset_budget_requires_current_full_ledger_evidence() {
        let mut quota = QuotaSnapshot {
            reset_credits_available: Some(2),
            provider_credits_observed_at_ms: Some(90),
            updated_at_ms: Some(100),
            ..Default::default()
        };
        assert_eq!(
            QuotaSpendResets::from_snapshot(&quota, 100, 20)
                .unwrap()
                .count,
            2
        );
        for observed in [None, Some(101), Some(79)] {
            quota.provider_credits_observed_at_ms = observed;
            assert!(QuotaSpendResets::from_snapshot(&quota, 100, 20).is_none());
        }
        quota.provider_credits_observed_at_ms = Some(90);
        quota.error = Some(super::super::QuotaErrorState::new("synthetic", 100));
        assert!(QuotaSpendResets::from_snapshot(&quota, 100, 20).is_none());
    }

    #[test]
    fn newer_full_ledger_updates_reset_count_despite_later_passive_window() {
        let mut current = QuotaSnapshot {
            reset_credits_available: Some(3),
            provider_credits_observed_at_ms: Some(10),
            updated_at_ms: Some(100),
            ..Default::default()
        };
        let full = QuotaSnapshot {
            reset_credits_available: Some(1),
            provider_credits_observed_at_ms: Some(90),
            updated_at_ms: Some(90),
            ..Default::default()
        };
        current.merge_observation(&full);
        assert_eq!(current.updated_at_ms, Some(100));
        assert_eq!(
            QuotaSpendResets::from_snapshot(&current, 100, 20)
                .unwrap()
                .count,
            1
        );
    }

    #[test]
    fn reset_weight_expires_without_reviving_empty_quota() {
        let deadline = QuotaSpendDeadline {
            expires_at_ms: 100 + 4 * 86_400_000,
            observed_at_ms: 100,
            valid_until_ms: 1_000,
            reset_credits: Some(QuotaSpendResets {
                count: 2,
                observed_at_ms: 100,
                valid_until_ms: 200,
            }),
        };
        assert_eq!(deadline.rotation_weight(5_000, 100), 5);
        assert_eq!(deadline.rotation_weight(0, 100), 1);
        assert_eq!(deadline.rotation_weight(5_000, 201), 1);
        assert_eq!(deadline.rotation_weight(5_000, 99), 1);
    }

    #[test]
    fn expiry_groups_keep_three_day_boundaries_and_reject_stale_evidence() {
        let now = 100;
        let deadline = QuotaSpendDeadline {
            expires_at_ms: now + EXPIRY_GROUP_MS,
            observed_at_ms: now,
            valid_until_ms: now + 1_000,
            reset_credits: None,
        };
        assert_eq!(deadline.expiry_group(now), Some(0));
        assert_eq!(
            QuotaSpendDeadline {
                expires_at_ms: deadline.expires_at_ms + 1,
                ..deadline
            }
            .expiry_group(now),
            Some(1)
        );
        assert_eq!(
            QuotaSpendDeadline {
                expires_at_ms: now + 4 * EXPIRY_GROUP_MS,
                ..deadline
            }
            .expiry_group(now),
            Some(3)
        );
        assert_eq!(deadline.expiry_group(now - 1), None);
        assert_eq!(deadline.expiry_group(now + 1_001), None);
        assert_eq!(
            QuotaSpendDeadline {
                expires_at_ms: now,
                ..deadline
            }
            .expiry_group(now),
            None
        );
    }

    #[test]
    fn expiry_weight_is_bounded_and_decreases_with_remaining_allowance() {
        let deadline = QuotaSpendDeadline {
            expires_at_ms: 100 + WEEK_MS / 7,
            observed_at_ms: 100,
            valid_until_ms: 1_000,
            reset_credits: None,
        };
        assert_eq!(deadline.weight(10_000, 100), 7);
        assert_eq!(deadline.weight(1_000, 100), 1);
        assert_eq!(deadline.weight(0, 100), 1);
        assert_eq!(deadline.weight(10_001, 100), 1);
        assert_eq!(
            QuotaSpendDeadline {
                expires_at_ms: 101,
                ..deadline
            }
            .weight(10_000, 100),
            8
        );
        assert_eq!(
            QuotaSpendDeadline {
                expires_at_ms: 100 + WEEK_MS * 2,
                ..deadline
            }
            .weight(10_000, 100),
            1
        );
    }

    #[test]
    fn expiry_requires_fresh_future_paid_entitlement_evidence() {
        let valid = Subscription {
            plan_type: Some("plus".into()),
            active_until_ms: Some(5_000),
            status: SubscriptionStatus::Active,
            updated_at_ms: Some(100),
        };
        let deadline = QuotaSpendDeadline::from_subscription(&valid, 100, 1_000).unwrap();
        assert_eq!(deadline.weight(10_000, 99), 1);
        assert_eq!(deadline.weight(10_000, 1_101), 1);
        assert_eq!(deadline.weight(10_000, 5_000), 1);
        for invalid in [
            Subscription {
                updated_at_ms: None,
                ..valid.clone()
            },
            Subscription {
                updated_at_ms: Some(101),
                ..valid.clone()
            },
            Subscription {
                updated_at_ms: Some(0),
                ..valid.clone()
            },
            Subscription {
                active_until_ms: None,
                ..valid.clone()
            },
            Subscription {
                active_until_ms: Some(100),
                ..valid.clone()
            },
            Subscription {
                plan_type: Some("free".into()),
                ..valid.clone()
            },
            Subscription {
                status: SubscriptionStatus::Unknown,
                ..valid.clone()
            },
            Subscription {
                status: SubscriptionStatus::Expired,
                ..valid.clone()
            },
            Subscription {
                status: SubscriptionStatus::Forbidden,
                ..valid.clone()
            },
        ] {
            assert!(QuotaSpendDeadline::from_subscription(&invalid, 100, 50).is_none());
        }
    }
}
