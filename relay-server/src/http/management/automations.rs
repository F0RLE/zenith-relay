use super::{store_error, validation_error, ManagementError};
use crate::state::{now_ms, AppState, ServerAccountRecord};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post};
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;
use zenith_relay_core::automations::{AccountSelector, WakeHistory, WakeModelPolicy, WakeTask};

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
    if task.id.trim().is_empty() {
        task.id = format!("wake_{}", uuid::Uuid::new_v4().simple());
    }
    let timestamp = now_ms();
    task.created_at_ms = timestamp;
    task.updated_at_ms = timestamp;
    task.validate()
        .map_err(|error| validation_error(format!("{error:?}")))?;
    state.store.save_wake_task(&task).map_err(store_error)?;
    Ok((StatusCode::CREATED, Json(task)))
}

pub async fn update_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(mut task): Json<WakeTask>,
) -> Result<Json<WakeTask>, ManagementError> {
    if !state
        .store
        .wake_tasks()
        .map_err(store_error)?
        .iter()
        .any(|value| value.id == id)
    {
        return Err(ManagementError::not_found(
            "wake_task_not_found",
            "wake task not found",
        ));
    }
    task.id = id;
    task.updated_at_ms = now_ms();
    task.validate()
        .map_err(|error| validation_error(format!("{error:?}")))?;
    state.store.save_wake_task(&task).map_err(store_error)?;
    Ok(Json(task))
}

pub async fn delete_wake_task(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    if !state.store.delete_wake_task(&id).map_err(store_error)? {
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
    let mut selected = match &task.account_selector {
        AccountSelector::AllEligible => accounts
            .iter()
            .filter(|account| account.enabled && !account.draining)
            .collect::<Vec<_>>(),
        AccountSelector::AccountIds(ids) => {
            let mut selected = Vec::with_capacity(ids.len());
            for account_id in ids {
                selected.push(
                    accounts
                        .iter()
                        .find(|account| account.id == *account_id)
                        .ok_or_else(|| {
                            ManagementError::validation(
                                "wake_account_missing",
                                "wake task references an unknown account",
                            )
                        })?,
                );
            }
            selected
        }
        AccountSelector::Tags(_) => Vec::new(),
    };
    selected.retain(|account| account.enabled && !account.draining);
    if let WakeModelPolicy::Explicit(model) = &task.model_policy {
        if matches!(task.account_selector, AccountSelector::AllEligible) {
            selected.retain(|account| account_supports_model(account, model));
        } else if selected
            .iter()
            .any(|account| !account_supports_model(account, model))
        {
            return Err(ManagementError::validation(
                "wake_model_unavailable",
                "wake model is unavailable for a selected account",
            ));
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
