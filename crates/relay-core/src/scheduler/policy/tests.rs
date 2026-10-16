use super::*;

fn policy() -> PoolRoutingPolicy {
    resolve_pool_routing(
        None,
        vec![
            (PoolMemberKind::Source, "primary".into(), 1_000_005, 3),
            (PoolMemberKind::Source, "reserve".into(), -1_000_000, 1),
            (PoolMemberKind::Account, "account".into(), 0, 1),
        ],
    )
}

#[test]
fn reconciles_inventory_without_losing_custom_order_or_limits() {
    let mut saved = policy();
    saved.mode = PoolRoutingMode::InOrder;
    saved.members.swap(0, 1);
    saved.members[0].max_concurrency = 2;
    let resolved = resolve_pool_routing(
        Some(&saved),
        vec![
            (PoolMemberKind::Source, "primary".into(), -5, 1),
            (PoolMemberKind::Account, "account".into(), -10, 1),
            (PoolMemberKind::Source, "new".into(), 2_000_000, 4),
        ],
    );
    assert_eq!(
        resolved
            .members
            .iter()
            .map(|m| m.id.as_str())
            .collect::<Vec<_>>(),
        ["account", "primary", "new"]
    );
    assert_eq!(resolved.members[0].max_concurrency, 2);
    assert_eq!(resolved.members[1].weight, 3);
    assert_eq!(resolved.mode, PoolRoutingMode::InOrder);
}

#[test]
fn rejects_stale_updates_and_invalid_or_changed_membership() {
    let current = policy();
    let mut updated_policy = current.clone();
    updated_policy.members.swap(0, 1);
    assert!(updated_policy
        .validate_update(&current, Some(&current))
        .is_ok());
    assert!(updated_policy
        .validate_update(&current, Some(&updated_policy))
        .is_err());
    updated_policy.members.pop();
    assert!(updated_policy
        .validate_update(&current, Some(&current))
        .is_err());
    let mut invalid = current.clone();
    invalid.members[0].weight = 0;
    assert!(invalid.validate().is_err());
    invalid = current.clone();
    invalid.members.push(current.members[0].clone());
    assert!(invalid.validate().is_err());
}

#[test]
fn remaps_portable_ids_atomically_and_rejects_collisions() {
    let mut saved = policy();
    let original = saved.clone();
    let mut ids: BTreeMap<_, _> = saved
        .members
        .iter()
        .map(|m| ((m.kind, m.id.clone()), format!("local-{}", m.id)))
        .collect();
    ids.remove(&(PoolMemberKind::Account, "account".into()));
    assert!(saved.remap_member_ids(&ids).is_err());
    assert_eq!(saved, original);
    ids.insert(
        (PoolMemberKind::Account, "account".into()),
        "local-account".into(),
    );
    saved.remap_member_ids(&ids).unwrap();
    assert!(saved.members.iter().all(|m| m.id.starts_with("local-")));
    let mut collision = original.clone();
    ids.insert(
        (PoolMemberKind::Source, "reserve".into()),
        "local-primary".into(),
    );
    assert!(collision.remap_member_ids(&ids).is_err());
    assert_eq!(collision, original);
}

#[test]
fn old_policy_upgrade_is_idempotent_and_preserves_order_weights_and_limits() {
    for mode in [
        PoolRoutingMode::Smart,
        PoolRoutingMode::InOrder,
        PoolRoutingMode::RoundRobin,
    ] {
        let legacy_policy = PoolRoutingPolicy {
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
        let upgraded = resolve_pool_routing(Some(&legacy_policy), inventory.clone());
        assert_eq!(upgraded.version, 2);
        assert_eq!(upgraded.members, legacy_policy.members);
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
