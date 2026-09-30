use super::*;
use crate::local_pool::models::LocalAccountRecord;
use std::collections::BTreeMap;
use zenith_relay_core::{
    accounts::{
        AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
    },
    quota::{QuotaSnapshot, Subscription},
    WireApi,
};

fn input(model_policy: WakeModelPolicy) -> WakeAutomationInput {
    WakeAutomationInput {
        name: "  Primary wake  ".into(),
        enabled: true,
        account_selector: AccountSelector::AllEligible,
        model_policy,
        trigger: WakeTrigger::QuotaFull,
        jitter_seconds: 60,
        max_attempts_per_cycle: 1,
    }
}

#[test]
fn command_input_cannot_define_a_prompt_or_unsupported_schedule() {
    let task = build_task(
        "wake_test".into(),
        input(WakeModelPolicy::Explicit(" gpt-test ".into())),
        10,
        20,
    )
    .unwrap();
    assert_eq!(task.name, "Primary wake");
    assert_eq!(task.trigger, WakeTrigger::QuotaFull);
    assert_eq!(task.fallback_schedule, None);
    assert_eq!(
        task.window_kinds,
        BTreeSet::from([QuotaWindowKind::Primary])
    );
    assert_eq!(
        task.model_policy,
        WakeModelPolicy::Explicit("gpt-test".into())
    );
    assert!(!serde_json::to_string(&task).unwrap().contains("prompt"));
}

#[test]
fn weekly_input_is_persisted_as_an_automatic_secondary_reset() {
    let mut input = input(WakeModelPolicy::Explicit("gpt-test".into()));
    input.trigger = WakeTrigger::Weekly;
    let task = build_task("weekly_reset".into(), input, 10, 20).unwrap();
    assert_eq!(task.trigger, WakeTrigger::Weekly);
    assert_eq!(
        task.window_kinds,
        BTreeSet::from([QuotaWindowKind::Secondary])
    );
    assert_eq!(task.model_policy, WakeModelPolicy::LightestSupported);
    assert_eq!(task.execution_policy, WakeExecutionPolicy::Automatic);
}

#[test]
fn legacy_manual_input_creates_an_automatic_task() {
    let input: WakeAutomationInput = serde_json::from_value(serde_json::json!({
        "name": "Primary wake",
        "accountSelector": { "kind": "all_eligible" },
        "modelPolicy": { "kind": "explicit", "value": "gpt-test" },
        "executionPolicy": "require_confirmation"
    }))
    .unwrap();
    let task = build_task("automatic_wake".into(), input, 10, 20).unwrap();
    assert_eq!(task.execution_policy, WakeExecutionPolicy::Automatic);
    assert_eq!(task.trigger, WakeTrigger::QuotaFull);
}

#[test]
fn explicit_model_must_be_available_for_every_selected_account() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-wake-model-{}",
        Uuid::new_v4().simple()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    state
        .store()
        .unwrap()
        .replace_accounts_and_keys(vec![account("account-1", &["gpt-test"])], Vec::new())
        .unwrap();
    let mut selected = input(WakeModelPolicy::Explicit("gpt-test".into()));
    selected.account_selector =
        AccountSelector::AccountIds(BTreeSet::from(["account-1".to_string()]));
    let task = build_task("wake_test".into(), selected, 10, 20).unwrap();
    validate_automation_targets(&task, &state).unwrap();
    assert_eq!(
        selected_automation_accounts(&task, &state).unwrap().len(),
        1
    );

    state
        .store()
        .unwrap()
        .replace_accounts_and_keys(
            vec![
                account("account-1", &["gpt-test"]),
                account("account-2", &["gpt-other"]),
            ],
            Vec::new(),
        )
        .unwrap();
    let mut unsupported = input(WakeModelPolicy::Explicit("gpt-test".into()));
    unsupported.account_selector = AccountSelector::AccountIds(BTreeSet::from([
        "account-1".to_string(),
        "account-2".to_string(),
    ]));
    let unsupported = build_task("wake_missing".into(), unsupported, 10, 20).unwrap();
    assert!(validate_automation_targets(&unsupported, &state).is_err());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

fn account(id: &str, models: &[&str]) -> LocalAccountRecord {
    LocalAccountRecord {
        account: AccountRecord {
            id: id.into(),
            label: id.into(),
            identity: AccountIdentity::from_hashed_parts(
                "openai",
                "chatgpt.com/backend-api/codex",
                &format!("identity-{id}"),
                &format!("secret-{id}"),
                "default",
                None,
            )
            .unwrap(),
            auth_mode: AccountAuthMode::OAuth,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            source_id: "openai_codex".into(),
            secret_refs: vec![format!("account:{id}")],
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
            token_generation: 1,
            token_updated_at_ms: Some(1),
            tags: BTreeSet::new(),
            enabled: true,
            in_pool: true,
            draining: false,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
        },
        provider_family: Some("openai".into()),
        purchase_cost_micro_usd: None,
        remote_location: None,
        wire_api: WireApi::Responses,
        models: models.iter().map(|model| (*model).to_string()).collect(),
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        client_auth_status: None,
        last_client_login_redirect_at_ms: None,
    }
}
