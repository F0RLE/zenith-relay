use super::*;

fn identity(kind: OAuthClientKind, workspace: &str, user: &str) -> AccountRouteIdentity {
    AccountRouteIdentity {
        client_kind: kind,
        provider_account_id: Some(workspace.into()),
        provider_user_id: Some(user.into()),
        original_login_email: Some("owner@example.test".into()),
    }
}

fn group(ids: &[&str], selected: &str) -> AccountRouteGroup {
    AccountRouteGroup {
        account_ids: ids.iter().map(|id| (*id).into()).collect(),
        selected_account_id: selected.into(),
        notes_account_id: None,
    }
}

#[test]
fn route_identity_preserves_client_principal_and_workspace_boundaries() {
    let native = identity(OAuthClientKind::Codex, "workspace", "user");
    assert!(native.same_owner(&identity(OAuthClientKind::ExcelBps, "workspace", "user")));
    assert!(!native.same_owner(&identity(OAuthClientKind::ExcelBps, "other", "user")));
    assert!(!native.same_owner(&identity(OAuthClientKind::ExcelBps, "workspace", "other")));
    assert!(!native.same_owner(&native));
    let mut missing = identity(OAuthClientKind::ExcelBps, "workspace", "user");
    missing.provider_user_id = None;
    assert!(!native.same_owner(&missing));
    missing.provider_account_id = None;
    assert!(!native.same_owner(&missing));
}

#[test]
fn siwc_requires_original_identity_and_cannot_bridge_other_workspaces() {
    let mut siwc = identity(OAuthClientKind::Siwc, "siwc-workspace", "siwc-user");
    siwc.original_login_email = Some(" OWNER@example.test ".into());
    let native = identity(OAuthClientKind::Codex, "workspace", "user");
    assert!(siwc.same_owner(&native));
    assert!(
        validate_account_route_connection(
            [siwc],
            &identity(OAuthClientKind::ExcelBps, "other", "user"),
        )
        .is_ok()
    );
    assert!(
        validate_account_route_connection(
            [identity(OAuthClientKind::Siwc, "siwc", "user"), native],
            &identity(OAuthClientKind::ExcelBps, "other", "user"),
        )
        .is_err()
    );
    for email in [None, Some(" ".into()), Some("other@example.test".into())] {
        let mut siwc = identity(OAuthClientKind::Siwc, "workspace", "user");
        siwc.original_login_email = email;
        assert!(!siwc.same_owner(&identity(OAuthClientKind::Codex, "workspace", "user")));
    }
    assert!(
        validate_account_route_connection(
            [identity(OAuthClientKind::Siwc, "workspace", "user")],
            &identity(OAuthClientKind::Siwc, "workspace", "user"),
        )
        .is_err()
    );
}

#[test]
fn route_group_validation_preserves_membership_and_note_ownership() {
    let valid = group(&["codex", "siwc", "bps"], "siwc");
    assert!(validate_account_route_groups(std::slice::from_ref(&valid)).is_ok());
    assert!(validate_account_route_groups(&[]).is_ok());
    for invalid in [
        group(&["codex"], "codex"),
        group(&["codex", "siwc", "bps", "extra"], "codex"),
        group(&["codex", "codex"], "codex"),
        group(&["", "siwc"], "siwc"),
        group(&["codex", "siwc"], "missing"),
        AccountRouteGroup {
            notes_account_id: Some("missing".into()),
            ..valid.clone()
        },
    ] {
        assert!(validate_account_route_groups(&[invalid]).is_err());
    }
    assert!(
        validate_account_route_groups(&[valid.clone(), group(&["codex", "other"], "other")])
            .is_err()
    );
    let with_notes = AccountRouteGroup {
        notes_account_id: Some("codex".into()),
        ..valid
    };
    let serialized = serde_json::to_value(&with_notes).unwrap();
    assert_eq!(serialized["selectedAccountId"], "siwc");
    assert_eq!(serialized["notesAccountId"], "codex");
    assert_eq!(
        serde_json::from_value::<AccountRouteGroup>(serialized).unwrap(),
        with_notes
    );
    let legacy: AccountRouteGroup = serde_json::from_value(serde_json::json!({
        "accountIds": ["codex", "bps"], "selectedAccountId": "bps"
    }))
    .unwrap();
    assert_eq!(legacy.notes_account_id, None);
}

#[test]
fn selecting_routes_is_pure_and_rejects_stale_or_associated_connections() {
    let original = AccountRouteGroup {
        notes_account_id: Some("codex".into()),
        ..group(&["codex", "siwc"], "siwc")
    };
    let groups = vec![original.clone(), group(&["other", "other-bps"], "other")];
    assert!(select_account_route_group(&groups, "codex", "siwc", false).is_err());
    assert!(select_account_route_group(&groups, "siwc", "other", true).is_err());
    assert!(select_account_route_group(&groups, "siwc", "missing", false).is_err());
    assert!(select_account_route_group(&groups, "siwc", "siwc", true).is_err());
    let selected = select_account_route_group(&groups, "siwc", "bps", true).unwrap();
    assert_eq!(selected.account_ids, ["codex", "siwc", "bps"]);
    assert_eq!(selected.selected_account_id, "bps");
    assert_eq!(selected.notes_account_id.as_deref(), Some("codex"));
    assert_eq!(groups[0], original);
    let new_group = select_account_route_group(&[], "codex", "bps", true).unwrap();
    assert_eq!(new_group, group(&["codex", "bps"], "bps"));
}

#[test]
fn pool_retargeting_keeps_weight_capacity_and_unrelated_members() {
    use crate::{PoolRoutingMember, PoolRoutingMode};

    let member = |kind, id: &str, weight, max_concurrency| PoolRoutingMember {
        kind,
        id: id.into(),
        weight,
        max_concurrency,
    };
    let original = PoolRoutingPolicy {
        mode: PoolRoutingMode::WeightedRoundRobin,
        members: vec![
            member(PoolMemberKind::Source, "codex", 3, 4),
            member(PoolMemberKind::Account, "codex", 7, 2),
            member(PoolMemberKind::Account, "bps", 99, 8),
            member(PoolMemberKind::Account, "unrelated", 5, 6),
        ],
        ..Default::default()
    };
    let mut policy = original.clone();
    group(&["codex", "siwc", "bps"], "bps").retarget_pool_routing(
        &mut policy,
        "codex",
        OAuthClientKind::ExcelBps,
    );
    assert_eq!(policy.mode, original.mode);
    assert_eq!(policy.version, original.version);
    assert_eq!(
        policy.members,
        [
            original.members[0].clone(),
            member(PoolMemberKind::Account, "bps", 7, 2),
            original.members[3].clone(),
        ]
    );
    group(&["codex", "siwc", "bps"], "codex").retarget_pool_routing(
        &mut policy,
        "bps",
        OAuthClientKind::Codex,
    );
    assert_eq!(
        policy.members,
        [original.members[0].clone(), original.members[3].clone()]
    );
}

#[test]
fn detaching_preserves_selection_and_never_assigns_notes_to_another_connection() {
    let mut groups = vec![AccountRouteGroup {
        notes_account_id: Some("codex".into()),
        ..group(&["codex", "siwc", "bps"], "bps")
    }];
    let original = groups.clone();
    for (selected, removed) in [("codex", "siwc"), ("bps", "bps"), ("bps", "missing")] {
        assert!(detach_account_route(&mut groups, selected, removed).is_err());
        assert_eq!(groups, original);
    }
    detach_account_route(&mut groups, "bps", "siwc").unwrap();
    assert_eq!(groups[0].notes_account_id.as_deref(), Some("codex"));
    detach_account_route(&mut groups, "bps", "codex").unwrap();
    assert!(groups.is_empty());

    let mut groups = original;
    detach_account_route(&mut groups, "bps", "codex").unwrap();
    assert_eq!(groups[0].notes_account_id, None);
    assert_eq!(groups[0].selected_account_id, "bps");
}
