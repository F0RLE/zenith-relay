use super::super::{find_account, runtime_error, store_error, vault_error, ManagementError};
use crate::state::{now_ms, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use std::sync::Arc;
use zenith_relay_core::{error_codes, protocol::TrashedAccountSummary};

pub async fn list_trash(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<TrashedAccountSummary>>, ManagementError> {
    Ok(Json(state.store.account_trash().map_err(store_error)?))
}

pub async fn trash_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let _wake = state.wake_lock.lock().await;
    let _configuration = state.configuration_lock.lock().await;
    let _build = state.lock_runtime_rebuild().await;
    let _credential = state.account_credential_lock.lock().await;
    find_account(&state, &id)?;
    let runtime = state.runtime().map_err(runtime_error)?;
    let _fence = runtime
        .as_ref()
        .and_then(|runtime| runtime.fence_candidate_dispatch(&id));
    if !state
        .store
        .trash_account(&id, now_ms())
        .map_err(store_error)?
    {
        return Err(ManagementError::not_found(
            error_codes::NOT_FOUND,
            "account not found",
        ));
    }
    if let Some(runtime) = runtime {
        runtime.remove_candidate(&id);
    }
    state.token_authority.remove(&id);
    Ok(StatusCode::NO_CONTENT)
}

pub async fn restore_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let _wake = state.wake_lock.lock().await;
    let _configuration = state.configuration_lock.lock().await;
    let _credential = state.account_credential_lock.lock().await;
    if !state
        .store
        .restore_trashed_account(&id)
        .map_err(store_error)?
    {
        return Err(ManagementError::not_found(
            error_codes::NOT_FOUND,
            "trashed account not found",
        ));
    }
    // Restored accounts are disabled and outside the pool. Enabling them uses
    // the existing validation/rebuild path, including current credentials.
    Ok(StatusCode::NO_CONTENT)
}

pub async fn purge_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let _wake = state.wake_lock.lock().await;
    let _configuration = state.configuration_lock.lock().await;
    let _credential = state.account_credential_lock.lock().await;
    let record = state
        .store
        .trashed_account(&id)
        .map_err(store_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::NOT_FOUND, "trashed account not found")
        })?;
    let secret = state.vault.load(&record.secret_ref).map_err(vault_error)?;
    state
        .vault
        .delete(&record.secret_ref)
        .map_err(vault_error)?;
    if let Err(error) = state.store.delete_account(&id) {
        if let Some(secret) = secret {
            state
                .vault
                .save(&record.secret_ref, &secret)
                .map_err(vault_error)?;
        }
        return Err(store_error(error));
    }
    state.token_authority.remove(&id);
    Ok(StatusCode::NO_CONTENT)
}
