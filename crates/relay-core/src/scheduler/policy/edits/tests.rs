use super::*;
use crate::PoolRoutingMember;
use serde_json::json;

fn policy() -> PoolRoutingPolicy {
    PoolRoutingPolicy {
        members: [
            (PoolMemberKind::Source, "s"),
            (PoolMemberKind::Account, "a"),
            (PoolMemberKind::Account, "b"),
        ]
        .into_iter()
        .map(|(kind, id)| PoolRoutingMember {
            kind,
            id: id.into(),
            weight: 4,
            max_concurrency: 2,
        })
        .collect(),
        ..PoolRoutingPolicy::default()
    }
}

fn apply(
    policy: &PoolRoutingPolicy,
    edits: serde_json::Value,
) -> Result<PoolRoutingPolicy, &'static str> {
    policy.apply_edits(&serde_json::from_value::<Vec<PoolRoutingEdit>>(edits).unwrap())
}

#[test]
fn subscription_expiry_edits_preserve_members_and_ignore_manual_moves() {
    let original = policy();
    let saved = apply(
        &original,
        json!([
            { "type": "mode", "mode": "subscription_expiry" },
            { "type": "move", "member": "account:b", "target": "source:s", "placement": "before" }
        ]),
    )
    .unwrap();
    assert_eq!(saved.mode, PoolRoutingMode::SubscriptionExpiry);
    assert_eq!(saved.members, original.members);
    assert_eq!(saved.rotation_preset(), None);
    assert!(saved.validate_activation().is_ok());
}

#[test]
fn presets_and_classification_share_backend_defaults() {
    for (preset, mode, expected) in [
        (
            "recommended",
            PoolRoutingMode::Automatic,
            RotationPreset::Recommended,
        ),
        ("even", PoolRoutingMode::RoundRobin, RotationPreset::Even),
        (
            "accounts_first",
            PoolRoutingMode::Failover,
            RotationPreset::AccountsFirst,
        ),
    ] {
        let saved = apply(&policy(), json!([{ "type": "preset", "preset": preset }])).unwrap();
        assert_eq!(saved.mode, mode);
        assert_eq!(saved.rotation_preset(), Some(expected));
        assert!(
            saved
                .members
                .iter()
                .all(|rule| rule.weight == 1 && rule.max_concurrency == 0)
        );
        let ids: Vec<_> = saved.members.iter().map(|rule| rule.id.as_str()).collect();
        assert_eq!(
            ids,
            if preset == "accounts_first" {
                vec!["a", "b", "s"]
            } else {
                vec!["s", "a", "b"]
            }
        );
    }
}

#[test]
fn edits_are_ordered_and_move_only_existing_members() {
    let saved = apply(&policy(), json!([
        { "type": "preset", "preset": "even" },
        { "type": "mode", "mode": "weighted_round_robin" },
        { "type": "member", "member": "account:b", "field": "weight", "value": 7 },
        { "type": "member", "member": "account:b", "field": "maxConcurrency", "value": 3 },
        { "type": "move", "member": "account:b", "target": "source:s", "placement": "before" },
        { "type": "move", "member": "source:s", "target": "account:a", "placement": "after" },
        { "type": "move", "member": "account:removed", "target": "source:s", "placement": "before" },
        { "type": "member", "member": "account:removed", "field": "weight", "value": 9 }
    ])).unwrap();
    assert_eq!(
        saved
            .members
            .iter()
            .map(|rule| rule.id.as_str())
            .collect::<Vec<_>>(),
        vec!["b", "a", "s"]
    );
    assert_eq!(
        (saved.members[0].weight, saved.members[0].max_concurrency),
        (7, 3)
    );
    assert_eq!(saved.rotation_preset(), None);
}

#[test]
fn automatic_moves_and_missing_targets_preserve_order() {
    let initial = policy();
    for mode in ["automatic", "failover"] {
        let saved = apply(&initial, json!([
            { "type": "mode", "mode": mode },
            { "type": "move", "member": "account:b", "target": "account:removed", "placement": "before" }
        ])).unwrap();
        assert_eq!(saved.members, initial.members);
    }
    assert_eq!(apply(&initial, json!([{ "type": "move", "member": "account:b", "target": "source:s", "placement": "before" }])).unwrap(), initial);
}

#[test]
fn invalid_values_and_modes_reject_the_whole_batch() {
    let initial = policy();
    for (field, value) in [("weight", 0), ("weight", 101), ("maxConcurrency", 1025)] {
        assert!(
            apply(
                &initial,
                json!([
                    { "type": "preset", "preset": "even" },
                    { "type": "member", "member": "account:a", "field": field, "value": value }
                ])
            )
            .is_err()
        );
    }
    assert!(apply(&initial, json!([{ "type": "mode", "mode": "smart" }])).is_err());
    assert_eq!(initial, policy());
}
