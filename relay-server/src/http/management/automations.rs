use super::{store_error, validation_error, ManagementError};
use crate::state::{now_ms, AppState, ServerAccountRecord};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;
use zenith_relay_core::automations::{
    AccountSelector, WakeCoordinator, WakeExecutionPolicy, WakeHistory, WakeModelPolicy, WakeTask,
};
use zenith_relay_core::quota::QuotaWindowKind;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/wake-tasks", get(list_wake_tasks).post(create_wake_task))
        .route(
            "/wake-tasks/{id}",
            patch(update_wake_task).delete(delete_wake_task),
        )
        .route("/wake-tasks/{id}/test", post(test_wake_task))
        .route("/wake-history", get(wake_history))
}

pub async fn list_wake_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<WakeTask>>, ManagementError> {
    Ok(Json(state.store.wake_tasks().map_err(store_error)?))
}

pub async fn create_wake_task(
    State(state): State<Arc<AppState>>,
    Json(mut task): Json<WakeTask>,
) -> Result<(StatusCode, Json<WakeTask>), ManagementError> {
    let _guard = state.wake_lock.lock().await;
    if task.id.trim().is_empty() {
        task.id = format!("wake_{}", uuid::Uuid::new_v4().simple());
    }
    let timestamp = now_ms();
    task.created_at_ms = timestamp;
    task.updated_at_ms = timestamp;
    task.window_kinds = [QuotaWindowKind::Primary].into();
    task.validate()
        .map_err(|error| validation_error(format!("{error:?}")))?;
    validate_remote_task(&task, &state.store.accounts().map_err(store_error)?)?;
    state.store.save_wake_task(&task).map_err(store_error)?;
    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn update_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut task): Json<WakeTask>,
) -> Result<Json<WakeTask>, ManagementError> {
    let _guard = state.wake_lock.lock().await;
    let current = state
        .store
        .wake_tasks()
        .map_err(store_error)?
        .into_iter()
        .find(|value| value.id == id)
        .ok_or_else(|| ManagementError::not_found("wake_task_not_found", "wake task not found"))?;
    task.id = id;
    task.created_at_ms = current.created_at_ms;
    task.updated_at_ms = now_ms();
    task.window_kinds = [QuotaWindowKind::Primary].into();
    task.validate()
        .map_err(|error| validation_error(format!("{error:?}")))?;
    validate_remote_task(&task, &state.store.accounts().map_err(store_error)?)?;
    let mut coordinator =
        WakeCoordinator::from_state(state.store.wake_state().map_err(store_error)?)
            .map_err(|error| store_error(error.to_string()))?;
    coordinator.remove_pending_for_task(&task.id, now_ms());
    state
        .store
        .save_wake_task_and_state(&task, coordinator.state())
        .map_err(store_error)?;
    Ok(Json(task))
}

pub async fn delete_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let _guard = state.wake_lock.lock().await;
    let mut coordinator =
        WakeCoordinator::from_state(state.store.wake_state().map_err(store_error)?)
            .map_err(|error| store_error(error.to_string()))?;
    coordinator.remove_pending_for_task(&id, now_ms());
    if !state
        .store
        .delete_wake_task_and_save_state(&id, coordinator.state())
        .map_err(store_error)?
    {
        return Err(ManagementError::not_found(
            "wake_task_not_found",
            "wake task not found",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WakeTestResult {
    task_id: String,
    status: &'static str,
    eligible_accounts: usize,
}

pub async fn test_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<WakeTestResult>, ManagementError> {
    let task = state
        .store
        .wake_tasks()
        .map_err(store_error)?
        .into_iter()
        .find(|value| value.id == id)
        .ok_or_else(|| ManagementError::not_found("wake_task_not_found", "wake task not found"))?;
    let accounts = state.store.accounts().map_err(store_error)?;
    validate_remote_task(&task, &accounts)?;
    let mut selected = selected_accounts(&task, &accounts)?;
    if let WakeModelPolicy::Explicit(model) = &task.model_policy {
        if matches!(&task.account_selector, AccountSelector::AllEligible) {
            selected.retain(|account| account_supports_model(account, model));
        }
    } else {
        selected.retain(|account| {
            account
                .models
                .iter()
                .any(|model| account_supports_model(account, model))
        });
    }
    Ok(Json(WakeTestResult {
        task_id: id,
        status: if selected.is_empty() {
            "no_eligible_accounts"
        } else {
            "ready"
        },
        eligible_accounts: selected.len(),
    }))
}

fn validate_remote_task(
    task: &WakeTask,
    accounts: &[ServerAccountRecord],
) -> Result<(), ManagementError> {
    if matches!(&task.account_selector, AccountSelector::Tags(_)) {
        return Err(ManagementError::validation(
            "wake_tags_unsupported",
            "tag-based wake tasks are not supported on the server",
        ));
    }
    if task.execution_policy != WakeExecutionPolicy::Automatic {
        return Err(ManagementError::validation(
            "wake_confirmation_unsupported",
            "manual wake confirmation is not supported on the server",
        ));
    }
    let selected = selected_accounts(task, accounts)?;
    let WakeModelPolicy::Explicit(model) = &task.model_policy else {
        return Ok(());
    };
    let valid = match &task.account_selector {
        AccountSelector::AllEligible => selected
            .iter()
            .any(|account| account_supports_model(account, model)),
        AccountSelector::AccountIds(_) => {
            !selected.is_empty()
                && selected
                    .iter()
                    .all(|account| account_supports_model(account, model))
        }
        AccountSelector::Tags(_) => false,
    };
    if !valid {
        return Err(ManagementError::validation(
            "wake_model_unavailable",
            "wake model is unavailable for the selected accounts",
        ));
    }
    Ok(())
}

fn selected_accounts<'a>(
    task: &WakeTask,
    accounts: &'a [ServerAccountRecord],
) -> Result<Vec<&'a ServerAccountRecord>, ManagementError> {
    let mut selected = match &task.account_selector {
        AccountSelector::AllEligible => accounts.iter().collect(),
        AccountSelector::AccountIds(ids) => ids
            .iter()
            .map(|account_id| {
                accounts
                    .iter()
                    .find(|account| account.id == *account_id)
                    .ok_or_else(|| {
                        ManagementError::validation(
                            "wake_account_missing",
                            "wake task references an unknown account",
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?,
        AccountSelector::Tags(_) => Vec::new(),
    };
    selected.retain(|account| account.enabled && !account.draining);
    Ok(selected)
}

fn account_supports_model(account: &ServerAccountRecord, model: &str) -> bool {
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
}

pub async fn wake_history(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<WakeHistory>>, ManagementError> {
    Ok(Json(
        state
            .store
            .wake_state()
            .map_err(store_error)?
            .history()
            .iter()
            .cloned()
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use zenith_relay_core::{
        automations::{WakeTask, WakeTrigger},
        quota::QuotaWindowKind,
    };

    fn task() -> WakeTask {
        WakeTask {
            id: "wake_test".into(),
            name: "Test".into(),
            enabled: true,
            account_selector: AccountSelector::AllEligible,
            window_kinds: BTreeSet::from([QuotaWindowKind::Primary]),
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
    fn server_rejects_wake_modes_it_cannot_execute() {
        assert!(validate_remote_task(&task(), &[]).is_ok());

        let mut confirmation = task();
        confirmation.execution_policy = WakeExecutionPolicy::RequireConfirmation;
        assert!(validate_remote_task(&confirmation, &[]).is_err());

        let mut tags = task();
        tags.account_selector = AccountSelector::Tags(BTreeSet::from(["work".into()]));
        assert!(validate_remote_task(&tags, &[]).is_err());

        let mut missing_account = task();
        missing_account.account_selector =
            AccountSelector::AccountIds(BTreeSet::from(["missing".into()]));
        assert!(validate_remote_task(&missing_account, &[]).is_err());
    }
}
