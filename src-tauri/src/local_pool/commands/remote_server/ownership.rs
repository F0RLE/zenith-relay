use super::{active_client, remote_error};
use crate::local_pool::{
    accounts::exports::{normalize_account_ids, normalize_one_account_id},
    error::{CommandError, ErrorCode, LocalPoolError},
    state::DesktopState,
};

use serde::{Deserialize, Serialize};

use tauri::{AppHandle, State};

use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::Feature;
use zenith_relay_core::protocol::RemoteAccountLocation;

mod execution;
mod reconcile;
mod transfer;

#[cfg(test)]
mod tests;

pub(super) use execution::ensure_no_pending_ownership_operation;
pub(super) use reconcile::reconcile_remote_account_locations;
pub(crate) use reconcile::{reconcile_saved_remote_ownership, recover_pending_remote_ownership};

pub(super) const REMOTE_MISSING_ERROR: &str = error_codes::REMOTE_MISSING;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MoveLocalAccountsToRemoteInput {
    pub account_ids: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveLocalAccountsToRemoteResult {
    pub moved: usize,
    pub remote_account_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReturnRemoteAccountToLocalInput {
    pub local_account_id: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReturnRemoteAccountToLocalResult {
    pub local_account_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForceActivateRemoteAccountLocallyInput {
    pub local_account_id: String,
    pub confirm_remote_may_still_be_running: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ForceActivateRemoteAccountLocallyResult {
    pub local_account_id: String,
}

pub(super) async fn move_local_accounts_to_remote(
    input: MoveLocalAccountsToRemoteInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<MoveLocalAccountsToRemoteResult, CommandError> {
    let account_ids = normalize_account_ids(input.account_ids)?;
    let _mutation = state.setup_guard().await;
    execution::ensure_no_pending_ownership_operation(&state)?;
    execution::ensure_local_accounts_transferable(&state, &account_ids)?;
    execution::emit_account_transfer_progress(&app, 0, &account_ids, "preparing");
    let Some((target, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    let capabilities = client.capabilities().await.map_err(remote_error)?;
    if !capabilities.supports(Feature::AccountBatchImport)
        || !capabilities.supports(Feature::AccountBatchImportCreationStatus)
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote server does not support safe account batch import rollback",
        )
        .into());
    }
    let operation = execution::new_move_operation(&target, account_ids);
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    let remote_account_ids =
        execution::execute_move_operation(&state, &client, &target, operation, Some(&app)).await?;

    Ok(MoveLocalAccountsToRemoteResult {
        moved: remote_account_ids.len(),
        remote_account_ids,
    })
}

pub(super) async fn return_remote_account_to_local(
    input: ReturnRemoteAccountToLocalInput,
    state: State<'_, DesktopState>,
) -> Result<ReturnRemoteAccountToLocalResult, CommandError> {
    let local_account_id = normalize_one_account_id(input.local_account_id)?;
    let _mutation = state.setup_guard().await;
    execution::ensure_no_pending_ownership_operation(&state)?;
    let remote_location = remote_location_of(&state, &local_account_id)?;
    let Some((target, client)) = active_client(&state)? else {
        return Err(
            LocalPoolError::new(ErrorCode::NotFound, "remote server is not connected").into(),
        );
    };
    if target.server_id != remote_location.server_id {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "connect the server that owns this account before returning it",
        )
        .into());
    }
    let capabilities = client.capabilities().await.map_err(remote_error)?;
    if !capabilities.supports(Feature::AccountExport) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote server does not support account return",
        )
        .into());
    }
    let operation =
        execution::new_return_operation(&target, local_account_id.clone(), remote_location);
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    execution::execute_return_operation(&state, &client, operation).await?;
    Ok(ReturnRemoteAccountToLocalResult { local_account_id })
}

pub(super) async fn force_activate_remote_account_locally(
    input: ForceActivateRemoteAccountLocallyInput,
    state: State<'_, DesktopState>,
) -> Result<ForceActivateRemoteAccountLocallyResult, CommandError> {
    if !input.confirm_remote_may_still_be_running {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "explicit confirmation is required for lost-server recovery",
        )
        .into());
    }
    let local_account_id = normalize_one_account_id(input.local_account_id)?;
    let _mutation = state.setup_guard().await;
    execution::ensure_no_pending_ownership_operation(&state)?;
    let remote_location = remote_location_of(&state, &local_account_id)?;
    // A missing management channel is the reason this explicit recovery path exists.
    if let Ok(Some((target, client))) = active_client(&state) {
        if target.server_id == remote_location.server_id
            && client.state().await.ok().is_some_and(|snapshot| {
                snapshot
                    .accounts
                    .iter()
                    .any(|account| account.id == remote_location.remote_account_id)
            })
        {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "the remote account is reachable; return it through the normal operation",
            )
            .into());
        }
    }
    let operation =
        execution::new_force_activation_operation(&remote_location, local_account_id.clone());
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    execution::execute_force_activation(&state, operation).await?;
    Ok(ForceActivateRemoteAccountLocallyResult { local_account_id })
}

fn remote_location_of(
    state: &State<'_, DesktopState>,
    local_account_id: &str,
) -> Result<RemoteAccountLocation, LocalPoolError> {
    state
        .store()?
        .account(local_account_id)
        .and_then(|account| account.remote_location.clone())
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account is not managed by a server",
            )
        })
}
