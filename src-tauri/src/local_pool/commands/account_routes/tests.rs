use super::*;
use crate::local_pool::{accounts::records::new_account_record, store::LocalPoolStore};
use base64::Engine;
use zenith_relay_core::{
    PoolMemberKind, PoolRoutingMember,
    accounts::AccountAuthMode,
    automations::{AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTrigger},
    quota::QuotaWindowKind,
};

fn same_route_owner(
    left_record: &LocalAccountRecord,
    left: &StoredCodexCredentials,
    right_record: &LocalAccountRecord,
    right: &StoredCodexCredentials,
) -> bool {
    route_identity(left_record, left).same_owner(&route_identity(right_record, right))
}

#[test]
fn linked_route_lookup_distinguishes_saved_members_from_other_groups() {
    let gateway = GatewaySettings {
        account_routes: vec![
            zenith_relay_core::accounts::AccountRouteGroup {
                account_ids: vec!["account_excel".into(), "account_siwc".into()],
                selected_account_id: "account_siwc".into(),
                notes_account_id: Some("account_excel".into()),
            },
            zenith_relay_core::accounts::AccountRouteGroup {
                account_ids: vec!["other_codex".into(), "other_excel".into()],
                selected_account_id: "other_codex".into(),
                notes_account_id: None,
            },
        ],
        ..Default::default()
    };
    assert!(route_is_linked(&gateway, "account_siwc", "account_excel"));
    assert!(route_is_linked(&gateway, "account_excel", "account_siwc"));
    assert!(!route_is_linked(&gateway, "account_siwc", "other_excel"));
    assert!(!route_is_linked(&gateway, "standalone", "account_excel"));
    assert!(!route_is_linked(&gateway, "account_siwc", "missing"));
}

#[test]
fn saved_route_candidates_filter_by_method_and_reject_ambiguous_matches() {
    let candidates = vec![
        ("synthetic-codex".into(), OAuthClientKind::Codex),
        ("synthetic-siwc".into(), OAuthClientKind::Siwc),
    ];
    assert_eq!(
        filter_route_candidates(candidates.clone(), None)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        filter_route_candidates(candidates.clone(), Some(OAuthClientKind::Siwc)).unwrap(),
        ["synthetic-siwc"]
    );
    assert!(
        filter_route_candidates(candidates.clone(), Some(OAuthClientKind::ExcelBps))
            .unwrap()
            .is_empty()
    );
    let duplicated = candidates
        .into_iter()
        .chain([("another-siwc".into(), OAuthClientKind::Siwc)]);
    assert_eq!(
        filter_route_candidates(duplicated, Some(OAuthClientKind::Siwc))
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}

#[test]
fn route_switch_waits_for_active_requests_and_pending_dispatches_only_in_its_group() {
    for (active, pending, blocked) in [(1, 0, true), (0, 1, true), (0, 0, false)] {
        let candidate: CandidateRuntimeSnapshot = serde_json::from_value(serde_json::json!({
            "candidateId": "account_codex", "kind": zenith_relay_core::CandidateKind::OAuthAccount, "available": true,
            "inFlight": pending, "activeRequestCount": active, "lastUsedAtMs": null,
            "nextRetryAtMs": null, "halfOpen": false, "dispatches": 0,
        }))
        .unwrap();
        assert_eq!(
            routes_have_active_requests(
                &["account_codex".into()],
                std::slice::from_ref(&candidate)
            ),
            blocked
        );
        assert!(!routes_have_active_requests(
            &["unrelated".into()],
            &[candidate]
        ));
    }
}

fn connection(
    id: &str,
    kind: OAuthClientKind,
    workspace: &str,
    user: &str,
    email: &str,
) -> (LocalAccountRecord, StoredCodexCredentials) {
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::json!({"email": email}).to_string());
    let credentials = StoredCodexCredentials::new(
        id,
        "synthetic-access".into(),
        Some("synthetic-refresh".into()),
        Some(format!("e30.{payload}.synthetic")),
        Some(60_000),
        1,
        1,
        Some(email.into()),
        Some(workspace.into()),
        Some(user.into()),
        None,
        Some("plus".into()),
        false,
    )
    .unwrap()
    .with_oauth_client_kind(kind);
    let mut account = new_account_record(
        &credentials,
        AccountAuthMode::OAuth,
        vec!["model-synthetic".into()],
        0,
        1,
    )
    .unwrap();
    account.account.id = id.into();
    (account, credentials)
}

fn records() -> Vec<LocalAccountRecord> {
    vec![
        connection(
            "account_codex",
            OAuthClientKind::Codex,
            "workspace",
            "user",
            "owner@example.test",
        )
        .0,
        connection(
            "account_excel",
            OAuthClientKind::ExcelBps,
            "workspace",
            "user",
            "owner@example.test",
        )
        .0,
    ]
}

#[test]
fn route_rollback_preserves_new_quota_generation_and_unrelated_accounts() {
    let mut previous = records();
    previous[0].account.in_pool = true;
    previous[0].image_pool_enabled = true;
    let mut current = previous.clone();
    current[0].image_pool_enabled = false;
    current[0].account.in_pool = !previous[0].account.in_pool;
    current[0].account.quota.limit_reached = true;
    current[0].account.token_generation = 77;
    current[1].weight = 42;
    restore_route_records(&mut current, &previous[..1]).unwrap();
    assert_eq!(current[0].account.in_pool, previous[0].account.in_pool);
    assert!(current[0].image_pool_enabled);
    assert!(current[0].account.quota.limit_reached);
    assert_eq!(current[0].account.token_generation, 77);
    assert_eq!(current[1].weight, 42);
    assert!(restore_route_records(&mut [], &previous[..1]).is_err());
}

#[test]
fn routes_require_same_principal_and_keep_workspace_boundaries() {
    let (native_record, native) = connection(
        "account_codex",
        OAuthClientKind::Codex,
        "workspace",
        "user",
        "owner@example.test",
    );
    for (workspace, user, allowed) in [
        ("workspace", "user", true),
        ("other", "user", false),
        ("workspace", "other", false),
    ] {
        let (record, credentials) = connection(
            "account_excel",
            OAuthClientKind::ExcelBps,
            workspace,
            user,
            "owner@example.test",
        );
        assert_eq!(
            same_route_owner(&native_record, &native, &record, &credentials),
            allowed
        );
    }
    assert!(!same_route_owner(
        &native_record,
        &native,
        &native_record,
        &native
    ));
}

#[test]
fn siwc_association_uses_original_identity_not_editable_login_notes() {
    let (record, native) = connection(
        "account_codex",
        OAuthClientKind::Codex,
        "workspace",
        "user",
        "owner@example.test",
    );
    let (siwc_record, siwc) = connection(
        "account_siwc",
        OAuthClientKind::Siwc,
        "registration-synthetic",
        "subject",
        "owner@example.test",
    );
    assert!(same_route_owner(&record, &native, &siwc_record, &siwc));
    let (other_record, original) = connection(
        "account_other",
        OAuthClientKind::Siwc,
        "registration-other",
        "other",
        "other@example.test",
    );
    let other = StoredCodexCredentials::new(
        "account_other",
        "synthetic-access".into(),
        Some("synthetic-refresh".into()),
        original.id_token().map(str::to_string),
        Some(60_000),
        1,
        1,
        Some("owner@example.test".into()),
        Some("registration-other".into()),
        Some("other".into()),
        None,
        None,
        false,
    )
    .unwrap()
    .with_oauth_client_kind(OAuthClientKind::Siwc);
    assert!(!same_route_owner(&record, &native, &other_record, &other));
}

#[test]
fn siwc_cannot_bridge_codex_and_bps_from_different_workspaces_or_users() {
    let (native_record, native) = connection(
        "account_codex",
        OAuthClientKind::Codex,
        "workspace",
        "user",
        "owner@example.test",
    );
    let (siwc_record, siwc) = connection(
        "account_siwc",
        OAuthClientKind::Siwc,
        "registration",
        "subject",
        "owner@example.test",
    );
    let connections = [(&siwc_record, siwc), (&native_record, native)];
    for (workspace, user, allowed) in [
        ("workspace", "user", true),
        ("other", "user", false),
        ("workspace", "other", false),
    ] {
        let (record, credentials) = connection(
            "account_excel",
            OAuthClientKind::ExcelBps,
            workspace,
            user,
            "owner@example.test",
        );
        assert!(same_route_owner(
            connections[0].0,
            &connections[0].1,
            &record,
            &credentials
        ));
        assert_eq!(
            validate_route_connection(&connections, &record, &credentials).is_ok(),
            allowed
        );
    }
    let (record, credentials) = connection(
        "account_duplicate",
        OAuthClientKind::Siwc,
        "registration",
        "subject",
        "owner@example.test",
    );
    assert!(validate_route_connection(&connections, &record, &credentials).is_err());
}

fn automation(selector: AccountSelector) -> WakeTask {
    WakeTask {
        id: "wake_synthetic".into(),
        name: "Synthetic wake".into(),
        enabled: true,
        account_selector: selector,
        window_kinds: [QuotaWindowKind::Primary].into(),
        model_policy: WakeModelPolicy::LightestSupported,
        trigger: WakeTrigger::QuotaFull,
        fallback_schedule: None,
        execution_policy: WakeExecutionPolicy::Automatic,
        jitter_seconds: 0,
        max_attempts_per_cycle: 1,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

#[test]
fn automations_follow_the_group_without_changing_unrelated_targets_or_selectors() {
    let destination = &records()[1];
    let group = vec![
        "account_codex".into(),
        "account_siwc".into(),
        "account_excel".into(),
    ];
    let selector = AccountSelector::AccountIds(
        [
            "account_codex".into(),
            "account_siwc".into(),
            "account_excel".into(),
            "unrelated".into(),
        ]
        .into(),
    );
    let mut tasks = vec![
        automation(selector),
        automation(AccountSelector::AllEligible),
        automation(AccountSelector::Tags(["work".into()].into())),
        automation(AccountSelector::AccountIds(["unrelated".into()].into())),
    ];
    let unrelated = tasks[1..].to_vec();
    select_automation_route(&mut tasks, &group, destination, OAuthClientKind::ExcelBps).unwrap();
    assert_eq!(
        tasks[0].account_selector,
        AccountSelector::AccountIds(["account_excel".into(), "unrelated".into()].into())
    );
    assert_eq!(tasks[0].created_at_ms, 1);
    assert!(tasks[0].updated_at_ms > 1);
    assert_eq!(&tasks[1..], unrelated.as_slice());
}

#[test]
fn an_enabled_explicit_automation_blocks_unsupported_or_excluded_route_models() {
    let mut accounts = records();
    let group = vec!["account_codex".into(), "account_excel".into()];
    let mut task = automation(AccountSelector::AccountIds(["account_codex".into()].into()));
    task.model_policy = WakeModelPolicy::Explicit("model-synthetic".into());
    let mut supported = vec![task.clone()];
    select_automation_route(
        &mut supported,
        &group,
        &accounts[1],
        OAuthClientKind::ExcelBps,
    )
    .unwrap();
    assert_eq!(
        supported[0].account_selector,
        AccountSelector::AccountIds(["account_excel".into()].into())
    );
    let mut tasks = vec![task.clone()];
    accounts[1].excluded_models.push("model-synthetic".into());
    assert!(
        select_automation_route(&mut tasks, &group, &accounts[1], OAuthClientKind::ExcelBps)
            .is_err()
    );
    assert_eq!(tasks, vec![task.clone()]);
    accounts[1].excluded_models.clear();
    accounts[1].models.clear();
    assert!(
        select_automation_route(&mut tasks, &group, &accounts[1], OAuthClientKind::ExcelBps)
            .is_err()
    );
    assert_eq!(tasks, vec![task]);
    tasks[0].enabled = false;
    select_automation_route(&mut tasks, &group, &accounts[1], OAuthClientKind::ExcelBps).unwrap();
    assert!(!tasks[0].enabled);
    assert_eq!(
        tasks[0].account_selector,
        AccountSelector::AccountIds(["account_excel".into()].into())
    );
}

#[test]
fn codex_selection_does_not_retarget_enabled_pool_automations() {
    let accounts = records();
    let group = vec!["account_codex".into(), "account_excel".into()];
    let task = automation(AccountSelector::AccountIds(["account_excel".into()].into()));
    let mut tasks = vec![task.clone()];
    assert!(
        select_automation_route(&mut tasks, &group, &accounts[0], OAuthClientKind::Codex).is_err()
    );
    assert_eq!(tasks, vec![task]);
    tasks[0].enabled = false;
    select_automation_route(&mut tasks, &group, &accounts[0], OAuthClientKind::Codex).unwrap();
    assert!(!tasks[0].enabled);
}

#[test]
fn selecting_a_route_preserves_quota_identity_and_one_pool_members_weight() {
    let mut accounts = records();
    accounts[0].account.in_pool = true;
    accounts[0].image_pool_enabled = true;
    accounts[0].weight = 7;
    accounts[1].account.in_pool = true;
    accounts[1].account.quota.limit_reached = true;
    let identity = accounts[1].account.identity.clone();
    let mut settings = GatewaySettings::default();
    settings
        .pool_routing
        .as_mut()
        .unwrap()
        .members
        .push(PoolRoutingMember {
            kind: PoolMemberKind::Account,
            id: "account_codex".into(),
            weight: 7,
            max_concurrency: 2,
        });
    select_records(
        &mut accounts,
        &mut settings,
        "account_codex",
        "account_excel",
        true,
        OAuthClientKind::ExcelBps,
    )
    .unwrap();
    assert!(!accounts[0].account.in_pool);
    assert!(accounts[1].account.in_pool);
    assert!(accounts.iter().all(|account| !account.image_pool_enabled));
    assert!(!settings.account_route_is_selected("account_codex"));
    assert!(settings.account_route_is_selected("account_excel"));
    assert_eq!(accounts[1].weight, 7);
    assert_eq!(accounts[1].account.identity, identity);
    assert!(accounts[1].account.quota.limit_reached);
    let policy = settings.pool_routing_for(&[], &accounts);
    assert_eq!(policy.members.len(), 1);
    assert_eq!(policy.members[0].id, "account_excel");
    assert_eq!(policy.members[0].max_concurrency, 2);
    settings.account_routes[0].notes_account_id = Some("account_codex".into());
    assert!(
        select_records(
            &mut accounts,
            &mut settings,
            "account_codex",
            "account_excel",
            false,
            OAuthClientKind::ExcelBps,
        )
        .is_err()
    );
    select_records(
        &mut accounts,
        &mut settings,
        "account_excel",
        "account_codex",
        false,
        OAuthClientKind::Codex,
    )
    .unwrap();
    assert!(!accounts[0].account.in_pool);
    assert!(!accounts[1].account.in_pool);
    assert!(accounts.iter().all(|account| !account.image_pool_enabled));
    assert!(settings.pool_routing.as_ref().unwrap().members.is_empty());
    assert_eq!(
        settings.account_routes[0].notes_account_id.as_deref(),
        Some("account_codex")
    );
}

#[test]
fn excel_siwc_round_trip_reuses_saved_connections_after_restart_and_stale_attach() {
    let root = crate::files::test_temp_root("excel-siwc-route-round-trip");
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let mut excel = records().remove(1);
    excel.account.in_pool = true;
    excel.weight = 7;
    excel.account.quota.limit_reached = true;
    let excel_identity = excel.account.identity.clone();
    let siwc = connection(
        "account_siwc",
        OAuthClientKind::Siwc,
        "siwc-workspace",
        "siwc-user",
        "owner@example.test",
    )
    .0;
    let mut accounts = vec![excel, siwc];
    let mut gateway = store.gateway().clone();
    select_records(
        &mut accounts,
        &mut gateway,
        "account_excel",
        "account_siwc",
        true,
        OAuthClientKind::Siwc,
    )
    .unwrap();
    gateway.account_routes[0].notes_account_id = Some("account_excel".into());
    store
        .replace_account_routes(accounts, gateway, store.automations().clone())
        .unwrap();
    drop(store);
    let store = LocalPoolStore::open(root.clone()).unwrap();
    let mut accounts = store.accounts().to_vec();
    let mut gateway = store.gateway().clone();
    assert!(route_is_linked(&gateway, "account_siwc", "account_excel"));
    let before = accounts.clone();
    assert!(
        select_records(
            &mut accounts,
            &mut gateway,
            "account_excel",
            "account_siwc",
            true,
            OAuthClientKind::Siwc,
        )
        .is_err()
    );
    assert_eq!(accounts, before);
    select_records(
        &mut accounts,
        &mut gateway,
        "account_siwc",
        "account_excel",
        true,
        OAuthClientKind::ExcelBps,
    )
    .unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(gateway.account_routes.len(), 1);
    let group = &gateway.account_routes[0];
    assert_eq!(group.account_ids, ["account_excel", "account_siwc"]);
    assert_eq!(group.selected_account_id, "account_excel");
    assert_eq!(group.notes_account_id.as_deref(), Some("account_excel"));
    assert!(accounts[0].account.in_pool);
    assert!(!accounts[1].account.in_pool);
    assert_eq!(accounts[0].account.identity, excel_identity);
    assert!(accounts[0].account.quota.limit_reached);
    assert_eq!(accounts[0].weight, 7);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn matching_siwc_registration_duplicates_join_the_existing_excel_group_without_losing_grants() {
    let root = crate::files::test_temp_root("duplicate-siwc-route");
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let mut connections = [
        connection(
            "siwc_live",
            OAuthClientKind::Siwc,
            "registration-one",
            "subject-one",
            "owner@example.test",
        ),
        connection(
            "account_excel",
            OAuthClientKind::ExcelBps,
            "workspace",
            "user",
            "owner@example.test",
        ),
        connection(
            "siwc_expired",
            OAuthClientKind::Siwc,
            "registration-two",
            "subject-two",
            "owner@example.test",
        ),
    ];
    connections[1].0.account.in_pool = true;
    connections[2].0.account.in_pool = true;
    connections[2].0.weight = 7;
    connections[2].0.account.auth_state = serde_json::from_value(serde_json::json!({
        "state": "requires_reauth", "reason": "token_rotation_recovery_required"
    }))
    .unwrap();
    let secrets = connections
        .iter()
        .map(|(_, credentials)| credentials.clone())
        .collect::<Vec<_>>();
    let mut accounts = connections
        .iter()
        .map(|(record, _)| record.clone())
        .collect::<Vec<_>>();
    let original_quota = accounts[1].account.quota.clone();
    let mut gateway = store.gateway().clone();
    gateway.account_routes.push(AccountRouteGroup {
        account_ids: vec!["siwc_live".into(), "account_excel".into()],
        selected_account_id: "account_excel".into(),
        notes_account_id: Some("siwc_live".into()),
    });
    gateway.pool_routing.as_mut().unwrap().members = vec![
        PoolRoutingMember {
            kind: PoolMemberKind::Account,
            id: "account_excel".into(),
            weight: 2,
            max_concurrency: 1,
        },
        PoolRoutingMember {
            kind: PoolMemberKind::Account,
            id: "siwc_expired".into(),
            weight: 7,
            max_concurrency: 3,
        },
    ];
    let group = route_association_plan(
        &gateway,
        "siwc_expired",
        "account_excel",
        connections
            .iter()
            .map(|(record, credentials)| (record, credentials)),
    )
    .unwrap();
    select_group_records(
        &mut accounts,
        &mut gateway,
        "siwc_expired",
        OAuthClientKind::ExcelBps,
        group,
    )
    .unwrap();
    assert_eq!(gateway.account_routes.len(), 1);
    assert_eq!(
        gateway.account_routes[0].account_ids,
        ["siwc_live", "account_excel", "siwc_expired"]
    );
    assert_eq!(
        gateway.account_routes[0].notes_account_id.as_deref(),
        Some("siwc_live")
    );
    assert_eq!(
        accounts
            .iter()
            .filter(|record| record.account.in_pool)
            .count(),
        1
    );
    assert!(accounts[1].account.in_pool);
    assert_eq!(accounts[1].account.quota, original_quota);
    assert_eq!(accounts[1].weight, 7);
    let members = &gateway.pool_routing.as_ref().unwrap().members;
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id, "account_excel");
    assert_eq!(members[0].max_concurrency, 3);
    store
        .replace_account_routes(accounts, gateway, store.automations().clone())
        .unwrap();
    drop(store);
    let store = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(store.accounts().len(), 3);
    assert_eq!(
        store
            .gateway()
            .account_route_group("siwc_expired")
            .unwrap()
            .selected_account_id,
        "account_excel"
    );
    assert!(
        connections
            .iter()
            .zip(&secrets)
            .all(|((_, credentials), before)| credentials.matches_snapshot(before))
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_client_candidates_prefer_a_usable_grant_only_within_the_same_group() {
    let candidates = preferred_route_candidates([
        SavedRouteCandidate {
            id: "expired".into(),
            kind: OAuthClientKind::Siwc,
            group_id: "group".into(),
            usable: false,
        },
        SavedRouteCandidate {
            id: "active".into(),
            kind: OAuthClientKind::Siwc,
            group_id: "group".into(),
            usable: true,
        },
        SavedRouteCandidate {
            id: "another".into(),
            kind: OAuthClientKind::Siwc,
            group_id: "another-group".into(),
            usable: true,
        },
    ]);
    assert_eq!(candidates[0].0, "active");
    assert_eq!(candidates[1].0, "another");
    assert!(filter_route_candidates(candidates, Some(OAuthClientKind::Siwc)).is_err());
}

#[test]
fn route_selection_is_atomic_restart_safe_and_rejects_duplicate_membership() {
    let root = crate::files::test_temp_root("account-routes");
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let mut accounts = records();
    accounts[0].account.in_pool = true;
    let mut settings = store.gateway().clone();
    let mut automations = store.automations().clone();
    automations
        .tasks
        .push(automation(AccountSelector::AccountIds(
            ["account_codex".into()].into(),
        )));
    automations
        .weekly_reset_fingerprints
        .insert("unrelated".into(), "synthetic-cycle".into());
    let ids = select_records(
        &mut accounts,
        &mut settings,
        "account_codex",
        "account_excel",
        true,
        OAuthClientKind::ExcelBps,
    )
    .unwrap();
    select_automation_route(
        &mut automations.tasks,
        &ids,
        &accounts[1],
        OAuthClientKind::ExcelBps,
    )
    .unwrap();
    store
        .replace_account_routes(accounts.clone(), settings.clone(), automations.clone())
        .unwrap();
    let mut invalid = accounts;
    invalid[0].account.in_pool = true;
    let mut invalid_automations = automations.clone();
    invalid_automations.tasks.clear();
    assert!(
        store
            .replace_account_routes(invalid, settings.clone(), invalid_automations)
            .is_err()
    );
    assert_eq!(store.automations(), &automations);
    drop(store);
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(store.gateway().account_routes, settings.account_routes);
    assert_eq!(store.automations(), &automations);
    assert_eq!(
        store.automations().tasks[0].account_selector,
        AccountSelector::AccountIds(["account_excel".into()].into())
    );
    assert!(!store.account("account_codex").unwrap().account.in_pool);
    let remaining = vec![store.account("account_codex").unwrap().clone()];
    store.replace_accounts_and_keys(remaining, vec![]).unwrap();
    assert!(store.gateway().account_routes.is_empty());
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
