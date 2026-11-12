use zenith_relay_core::quota::QuotaSpendDeadline;

/// A saved reset is reserve capacity, unless it or its entitlement would expire.
pub(super) fn should_redeem(
    ordinary_capacity_available: Option<bool>,
    natural_reset_at_ms: Option<u64>,
    credit_expires_at_ms: Option<u64>,
    subscription_deadline: Option<QuotaSpendDeadline>,
    next_check_at_ms: u64,
    now_ms: u64,
) -> bool {
    if credit_expires_at_ms.is_some_and(|expiry| expiry <= now_ms)
        || natural_reset_at_ms.is_some_and(|reset| reset <= next_check_at_ms)
    {
        return false;
    }
    if ordinary_capacity_available == Some(false) {
        return true;
    }
    credit_expires_at_ms.is_some_and(|expiry| expiry <= next_check_at_ms)
        || subscription_deadline.is_some_and(|deadline| {
            deadline.weight(10_000, now_ms) > 1
                && natural_reset_at_ms.is_none_or(|reset| deadline.expires_at_ms < reset)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expiring_entitlement_can_use_verified_resets_before_natural_recovery() {
        let deadline = QuotaSpendDeadline {
            expires_at_ms: 5_000,
            observed_at_ms: 1_000,
            valid_until_ms: 3_000,
        };
        for capacity in [Some(true), None] {
            assert!(should_redeem(
                capacity,
                Some(10_000),
                None,
                Some(deadline),
                2_000,
                1_000
            ));
            assert!(should_redeem(
                capacity,
                None,
                Some(9_000),
                Some(deadline),
                2_000,
                1_000
            ));
            assert!(
                !should_redeem(capacity, Some(4_000), None, Some(deadline), 2_000, 1_000),
                "natural quota recovers before the subscription ends"
            );
            assert!(!should_redeem(
                capacity,
                Some(1_500),
                None,
                Some(deadline),
                2_000,
                1_000
            ));
            assert!(
                !should_redeem(
                    capacity,
                    Some(10_000),
                    Some(1_000),
                    Some(deadline),
                    2_000,
                    1_000
                ),
                "an expired reset remains unusable"
            );
        }
    }

    #[test]
    fn stale_or_expired_entitlements_cannot_override_the_reserve() {
        for deadline in [
            QuotaSpendDeadline {
                expires_at_ms: 5_000,
                observed_at_ms: 1_001,
                valid_until_ms: 3_000,
            },
            QuotaSpendDeadline {
                expires_at_ms: 5_000,
                observed_at_ms: 900,
                valid_until_ms: 999,
            },
            QuotaSpendDeadline {
                expires_at_ms: 1_000,
                observed_at_ms: 900,
                valid_until_ms: 3_000,
            },
            QuotaSpendDeadline {
                expires_at_ms: u64::MAX,
                observed_at_ms: 900,
                valid_until_ms: 3_000,
            },
        ] {
            assert!(!should_redeem(
                Some(true),
                Some(u64::MAX),
                None,
                Some(deadline),
                2_000,
                1_000
            ));
        }
    }

    #[test]
    fn available_or_unknown_pool_capacity_preserves_the_reset() {
        for available in [Some(true), None] {
            assert!(!should_redeem(
                available,
                Some(10_000),
                None,
                None,
                2_000,
                1_000
            ));
            assert!(!should_redeem(
                available,
                Some(10_000),
                Some(9_000),
                None,
                2_000,
                1_000
            ));
        }
        assert!(should_redeem(
            Some(false),
            Some(10_000),
            None,
            None,
            2_000,
            1_000
        ));
    }

    #[test]
    fn an_expiring_reset_can_restore_an_exhausted_week_before_it_is_lost() {
        assert!(should_redeem(
            Some(true),
            Some(10_000),
            Some(1_500),
            None,
            2_000,
            1_000
        ));
        assert!(should_redeem(None, None, Some(1_500), None, 2_000, 1_000));
        assert!(!should_redeem(
            Some(false),
            Some(10_000),
            Some(1_000),
            None,
            2_000,
            1_000
        ));
    }

    #[test]
    fn imminent_or_unverified_natural_recovery_preserves_the_reset() {
        for natural in [500, 1_000, 1_500, 2_000] {
            assert!(!should_redeem(
                Some(false),
                Some(natural),
                Some(1_700),
                None,
                2_000,
                1_000
            ));
        }
    }
}
