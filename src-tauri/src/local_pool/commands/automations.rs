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
    unix_time_ms as current_time_ms, ModelRules,
};

type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeAutomationInput {
    name: String,
    #[serde(default = "enabled_by_default")]
    enabled: bool,
    account_selector: AccountSelector,
    model_policy: WakeModelPolicy,
    #[serde(default = "quota_full_trigger")]
    trigger: WakeTrigger,
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
    let mut automation_records = state.store()?.automations().clone();
    validate_automation_targets(&task, &state)?;
    automation_records.tasks.push(task);
    state.store()?.replace_automations(automation_records)?;
    state.snapshot().await.map_err(Into::into)
}

#[tauri::command]
pub async fn update_quota_wake_automation(
    task_id: String,
    input: WakeAutomationInput,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    let existing_task = state
        .store()?
        .automations()
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "automation not found"))?;
    let updated = build_task(
        existing_task.id.clone(),
        input,
        existing_task.created_at_ms,
        current_time_ms(),
    )?;
    validate_automation_targets(&updated, &state)?;
    state.remove_pending_wakes_for_task(&updated.id)?;
    let automation_records = state.store()?.automations().clone();
    let tasks = automation_records
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
        state: automation_records.state,
        weekly_reset_fingerprints: automation_records.weekly_reset_fingerprints,
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
    let existing_task = state
        .store()?
        .automations()
        .tasks
        .iter()
        .find(|task| task.id == task_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "automation not found"))?;
    if existing_task.enabled == enabled {
        return state.snapshot().await.map_err(Into::into);
    }
    if !enabled {
        state.remove_pending_wakes_for_task(&task_id)?;
    }
    let mut automation_records = state.store()?.automations().clone();
    let task = automation_records
        .tasks
        .iter_mut()
        .find(|task| task.id == task_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "automation not found"))?;
    task.enabled = enabled;
    task.updated_at_ms = current_time_ms();
    state.store()?.replace_automations(automation_records)?;
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
    let mut automation_records = state.store()?.automations().clone();
    let task_count_before_delete = automation_records.tasks.len();
    automation_records.tasks.retain(|task| task.id != task_id);
    debug_assert!(automation_records.tasks.len() < task_count_before_delete);
    state.store()?.replace_automations(automation_records)?;
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
    let is_weekly_reset = input.trigger == WakeTrigger::Weekly;
    let task = WakeTask {
        id,
        name: input.name.trim().to_string(),
        enabled: input.enabled,
        account_selector: input.account_selector,
        window_kinds: BTreeSet::from([if is_weekly_reset {
            QuotaWindowKind::Secondary
        } else {
            QuotaWindowKind::Primary
        }]),
        model_policy: if is_weekly_reset {
            WakeModelPolicy::LightestSupported
        } else {
            trim_model_policy(input.model_policy)
        },
        trigger: input.trigger,
        fallback_schedule: None,
        execution_policy: WakeExecutionPolicy::Automatic,
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
            .effective_models()
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(model))
            && ModelRules::from_allow_deny(&account.allowed_models, &account.excluded_models)
                .allows(model)
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
        AccountSelector::AccountIds(account_ids) => account_ids
            .iter()
            .map(|account_id| {
                store.account(account_id).cloned().ok_or_else(|| {
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

fn enabled_by_default() -> bool {
    true
}

fn default_attempt_limit() -> u8 {
    1
}

fn quota_full_trigger() -> WakeTrigger {
    WakeTrigger::QuotaFull
}
#[cfg(test)]
mod tests;
