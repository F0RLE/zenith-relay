use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    models::{AutomationRecords, LocalPoolSnapshot},
    state::DesktopState,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tauri::State;
use uuid::Uuid;
use zenith_relay_core::{
    automations::{AccountSelector, WakeExecutionPolicy, WakeModelPolicy, WakeTask, WakeTrigger},
    quota::QuotaWindowKind,
};

type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeAutomationInput {
    name: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    account_selector: AccountSelector,
    window_kinds: BTreeSet<QuotaWindowKind>,
    model_policy: WakeModelPolicy,
    #[serde(default = "automatic_execution")]
    execution_policy: WakeExecutionPolicy,
    #[serde(default)]
    jitter_seconds: u32,
    #[serde(default = "default_attempt_limit")]
    max_attempts_per_cycle: u8,
}

#[tauri::command]
pub async fn create_quota_wake_automation(
    input: WakeAutomationInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let now_ms = current_time_ms();
    let task = build_task(
        format!("wake_{}", Uuid::new_v4().simple()),
        input,
        now_ms,
        now_ms,
    )?;
    let mut records = state.store()?.automations().clone();
    validate_automation_targets(&task, &state)?;
    records.tasks.push(task);
    state.store()?.replace_automations(records)?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn update_quota_wake_automation(
    task_id: String,
    input: WakeAutomationInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let current = state
        .store()?
        .automations()
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "automation not found"))?;
    let updated = build_task(
        current.id.clone(),
        input,
        current.created_at_ms,
        current_time_ms(),
    )?;
    validate_automation_targets(&updated, &state)?;
    state.remove_pending_wakes_for_task(&updated.id)?;
    let records = state.store()?.automations().clone();
    let tasks = records
        .tasks
        .into_iter()
        .map(|task| {
            if task.id == updated.id {
                updated.clone()
            } else {
                task
            }
        })
        .collect();
    state.store()?.replace_automations(AutomationRecords {
        tasks,
        state: records.state,
    })?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn set_quota_wake_automation_enabled(
    task_id: String,
    enabled: bool,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let current = state
        .store()?
        .automations()
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "automation not found"))?;
    if current.enabled == enabled {
        return state.snapshot().await.map_err(Into::into);
    }
    if !enabled {
        state.remove_pending_wakes_for_task(&task_id)?;
    }
    let mut records = state.store()?.automations().clone();
    let task = records
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "automation not found"))?;
    task.enabled = enabled;
    task.updated_at_ms = current_time_ms();
    state.store()?.replace_automations(records)?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn delete_quota_wake_automation(
    task_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    if !state
        .store()?
        .automations()
        .tasks
        .iter()
        .any(|task| task.id == task_id)
    {
        return Err(LocalPoolError::new(ErrorCode::NotFound, "automation not found").into());
    }
    state.remove_pending_wakes_for_task(&task_id)?;
    let mut records = state.store()?.automations().clone();
    let before = records.tasks.len();
    records.tasks.retain(|task| task.id != task_id);
    debug_assert!(records.tasks.len() < before);
    state.store()?.replace_automations(records)?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn run_due_quota_wake_confirmations(
    max_claims: Option<u8>,
    state: State<'_, DesktopState>,
) -> CommandResult<usize> {
    crate::local_pool::background::run_due_confirmation_wakes(
        &state,
        usize::from(max_claims.unwrap_or(1).clamp(1, 2)),
    )
    .await
    .map_err(Into::into)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeAutomationTestResult {
    task_id: String,
    status: &'static str,
    eligible_accounts: usize,
}

#[tauri::command]
pub async fn test_quota_wake_automation(
    task_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<WakeAutomationTestResult> {
    let task = state
        .store()?
        .automations()
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "automation not found"))?;
    validate_automation_targets(&task, &state)?;
    let eligible_accounts = selected_automation_accounts(&task, &state)?.len();
    Ok(WakeAutomationTestResult {
        task_id,
        status: if eligible_accounts == 0 {
            "no_eligible_accounts"
        } else {
            "ready"
        },
        eligible_accounts,
    })
}

fn build_task(
    id: String,
    input: WakeAutomationInput,
    created_at_ms: u64,
    updated_at_ms: u64,
) -> Result<WakeTask, CommandError> {
    let task = WakeTask {
        id,
        name: input.name.trim().to_string(),
        enabled: input.enabled,
        account_selector: input.account_selector,
        window_kinds: input.window_kinds,
        model_policy: trim_model_policy(input.model_policy),
        trigger: WakeTrigger::QuotaFull,
        fallback_schedule: None,
        execution_policy: input.execution_policy,
        jitter_seconds: input.jitter_seconds,
        max_attempts_per_cycle: input.max_attempts_per_cycle,
        created_at_ms,
        updated_at_ms,
    };
    task.validate().map_err(|_| {
        CommandError::from(LocalPoolError::new(
            ErrorCode::InvalidState,
            "automation settings are invalid",
        ))
    })?;
    Ok(task)
}

fn validate_automation_targets(task: &WakeTask, state: &DesktopState) -> Result<(), CommandError> {
    let selected = selected_automation_accounts(task, state)?;
    let WakeModelPolicy::Explicit(model) = &task.model_policy else {
        return Ok(());
    };
    let supports_model = |account: &crate::local_pool::models::LocalAccountRecord| {
        account
            .models
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(model))
            && (account.allowed_models.is_empty()
                || account
                    .allowed_models
                    .iter()
                    .any(|allowed| allowed.eq_ignore_ascii_case(model)))
            && !account
                .excluded_models
                .iter()
                .any(|excluded| excluded.eq_ignore_ascii_case(model))
    };
    let valid = match &task.account_selector {
        AccountSelector::AllEligible => selected.iter().any(supports_model),
        AccountSelector::AccountIds(_) | AccountSelector::Tags(_) => {
            !selected.is_empty() && selected.iter().all(supports_model)
        }
    };
    if !valid {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "explicit wake model is unavailable for the selected accounts",
        )
        .into());
    }
    Ok(())
}

fn selected_automation_accounts(
    task: &WakeTask,
    state: &DesktopState,
) -> Result<Vec<crate::local_pool::models::LocalAccountRecord>, CommandError> {
    let store = state.store()?;
    let mut selected = match &task.account_selector {
        AccountSelector::AllEligible => store.accounts().to_vec(),
        AccountSelector::AccountIds(ids) => ids
            .iter()
            .map(|id| {
                store.account(id).cloned().ok_or_else(|| {
                    LocalPoolError::new(
                        ErrorCode::InvalidState,
                        "automation contains an unknown account",
                    )
                    .into()
                })
            })
            .collect::<Result<Vec<_>, CommandError>>()?,
        AccountSelector::Tags(tags) => store
            .accounts()
            .iter()
            .filter(|account| !tags.is_disjoint(&account.account.tags))
            .cloned()
            .collect(),
    };
    selected.retain(|account| account.account.enabled && !account.account.draining);
    Ok(selected)
}

fn trim_model_policy(policy: WakeModelPolicy) -> WakeModelPolicy {
    match policy {
        WakeModelPolicy::Explicit(model) => WakeModelPolicy::Explicit(model.trim().to_string()),
        WakeModelPolicy::LightestSupported => WakeModelPolicy::LightestSupported,
    }
}

fn current_time_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or_default()
}

fn enabled_by_default() -> bool {
    true
}

fn automatic_execution() -> WakeExecutionPolicy {
    WakeExecutionPolicy::Automatic
}

fn default_attempt_limit() -> u8 {
    1
}

#[cfg(test)]
mod tests {
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
            window_kinds: BTreeSet::from([QuotaWindowKind::Primary]),
            model_policy,
            execution_policy: WakeExecutionPolicy::Automatic,
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
            task.model_policy,
            WakeModelPolicy::Explicit("gpt-test".into())
        );
        assert!(!serde_json::to_string(&task).unwrap().contains("prompt"));
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
                draining: false,
                created_at_ms: 1,
                last_used_at_ms: None,
                last_error_code: None,
            },
            wire_api: WireApi::Responses,
            models: models.iter().map(|model| (*model).to_string()).collect(),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            cooldowns: BTreeMap::new(),
            consecutive_failures: 0,
        }
    }
}
