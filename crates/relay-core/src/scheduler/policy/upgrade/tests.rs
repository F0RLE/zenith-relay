use super::*;
use crate::{resolve_pool_routing, PoolMemberKind, PoolRoutingMember};

#[test]
fn old_policy_upgrade_is_idempotent_and_preserves_order_weights_and_limits() {
    for mode in [
        PoolRoutingMode::Smart,
        PoolRoutingMode::InOrder,
        PoolRoutingMode::RoundRobin,
    ] {
        let old = PoolRoutingPolicy {
            version: 1,
            mode,
            members: vec![PoolRoutingMember {
                kind: PoolMemberKind::Source,
                id: "source".into(),
                weight: 9,
                max_concurrency: 7,
            }],
        };
        let inventory = vec![(PoolMemberKind::Source, "source".into(), -1_000_000, 2)];
        let upgraded = resolve_pool_routing(Some(&old), inventory.clone());
        assert_eq!(upgraded.version, 2);
        assert_eq!(upgraded.members, old.members);
        assert_eq!(
            upgraded.mode,
            if mode == PoolRoutingMode::Smart {
                PoolRoutingMode::Automatic
            } else {
                mode
            }
        );
        assert!(upgraded.validate_activation().is_ok());
        assert_eq!(resolve_pool_routing(Some(&upgraded), inventory), upgraded);
    }
}

#[test]
fn pre_policy_inventory_upgrades_and_invalid_values_still_fail_validation() {
    let policy = resolve_pool_routing(
        None,
        vec![(PoolMemberKind::Account, "account".into(), 0, 4)],
    );
    assert!(policy.is_current_rotation());
    assert_eq!(policy.members[0].weight, 4);
    let invalid = resolve_pool_routing(
        None,
        vec![(PoolMemberKind::Source, "invalid".into(), 0, 500)],
    );
    assert!(invalid.validate_activation().is_err());
    let future = PoolRoutingPolicy {
        version: 255,
        ..Default::default()
    };
    assert!(resolve_pool_routing(Some(&future), vec![])
        .validate_activation()
        .is_err());
}
