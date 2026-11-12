use super::Subscription;

const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const MAX_EXPIRY_WEIGHT: u32 = 8;

/// Fresh entitlement evidence used only to bias useful, independent work.
/// A deadline is neither available quota nor authorization to redeem a reset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QuotaSpendDeadline {
    pub expires_at_ms: u64,
    pub observed_at_ms: u64,
    pub valid_until_ms: u64,
}

impl QuotaSpendDeadline {
    pub fn from_subscription(
        subscription: &Subscription,
        now_ms: u64,
        stale_after_ms: u64,
    ) -> Option<Self> {
        Some(Self {
            expires_at_ms: subscription.active_deadline_ms(now_ms, stale_after_ms)?,
            observed_at_ms: subscription.updated_at_ms?,
            valid_until_ms: subscription.updated_at_ms?.saturating_add(stale_after_ms),
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::SubscriptionStatus;

    #[test]
    fn expiry_weight_is_bounded_and_decreases_with_remaining_allowance() {
        let deadline = QuotaSpendDeadline {
            expires_at_ms: 100 + WEEK_MS / 7,
            observed_at_ms: 100,
            valid_until_ms: 1_000,
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
