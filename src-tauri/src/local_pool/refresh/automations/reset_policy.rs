/// A saved reset is reserve capacity, separate from currently spendable quota.
pub(super) fn should_redeem(
    ordinary_capacity_available: Option<bool>,
    natural_reset_at_ms: Option<u64>,
    credit_expires_at_ms: Option<u64>,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_or_unknown_pool_capacity_preserves_the_reset() {
        for available in [Some(true), None] {
            assert!(!should_redeem(available, Some(10_000), None, 2_000, 1_000));
            assert!(!should_redeem(
                available,
                Some(10_000),
                Some(9_000),
                2_000,
                1_000
            ));
        }
        assert!(should_redeem(Some(false), Some(10_000), None, 2_000, 1_000));
    }

    #[test]
    fn an_expiring_reset_can_restore_an_exhausted_week_before_it_is_lost() {
        assert!(should_redeem(
            Some(true),
            Some(10_000),
            Some(1_500),
            2_000,
            1_000
        ));
        assert!(should_redeem(None, None, Some(1_500), 2_000, 1_000));
        assert!(!should_redeem(
            Some(false),
            Some(10_000),
            Some(1_000),
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
                2_000,
                1_000
            ));
        }
    }
}
