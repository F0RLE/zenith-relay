use super::super::{find_account, runtime_error, store_error, vault_error, ManagementError};
use crate::state::{now_ms, AppState};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use std::sync::Arc;
use zenith_relay_core::error_codes;

pub async fn delete_account(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<StatusCode, ManagementError> {
    let _wake_guard = state.wake_lock.lock().await;
    let configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let credential = state.account_credential_lock.lock().await;
    let account_record = find_account(&state, &id)?;
    let secret = state
        .vault
        .load(&account_record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(
                error_codes::ACCOUNT_SECRET_MISSING,
                "account secret missing",
            )
        })?;
    // Close the old login before either durable store or vault changes. Hold
    // the fence through replacement/rollback; only already-started attempts
    // may settle after deletion begins.
    let previous_runtime = state.runtime().map_err(runtime_error)?;
    let _dispatch_fence = previous_runtime
        .as_ref()
        .and_then(|runtime| runtime.fence_candidate_dispatch(&id));
    state.store.delete_account(&id).map_err(store_error)?;
    if let Err(error) = state.vault.delete(&account_record.secret_ref) {
        drop(credential);
        drop(configuration);
        build
            .rollback_and_rebuild(&state, || state.store.save_account(&account_record))
            .await
            .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(vault_error(error));
    }
    state.token_authority.remove(&id);
    if let Some(runtime) = previous_runtime.as_ref() {
        runtime.remove_candidate(&id);
    }
    drop(credential);
    drop(configuration);
    if let Err(error) = build.rebuild(&state).await {
        build
            .rollback_and_rebuild(&state, || {
                state.vault.save(&account_record.secret_ref, &secret)?;
                state.store.save_account(&account_record)
            })
            .await
            .map_err(|restore| runtime_error(format!("{error}; {restore}")))?;
        return Err(runtime_error(error));
    }
    drop(build);
    state
        .store
        .remove_account_from_wake_tasks(&id, now_ms())
        .map_err(store_error)?;
    Ok(StatusCode::NO_CONTENT)
}
