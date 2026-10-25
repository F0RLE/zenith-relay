use super::*;
use crate::local_pool::{accounts::records::new_account_record, store::LocalPoolStore};
use base64::Engine;
use zenith_relay_core::{
    accounts::AccountAuthMode,
    automations::{AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTrigger},
    quota::QuotaWindowKind,
    PoolMemberKind, PoolRoutingMember,
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
    assert!(select_records(
        &mut accounts,
        &mut settings,
        "account_codex",
        "account_excel",
        false,
        OAuthClientKind::ExcelBps,
    )
    .is_err());
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
fn route_selection_is_atomic_restart_safe_and_rejects_duplicate_membership() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-account-routes-{}",
        uuid::Uuid::new_v4()
    ));
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
    assert!(store
        .replace_account_routes(invalid, settings.clone(), invalid_automations)
        .is_err());
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
