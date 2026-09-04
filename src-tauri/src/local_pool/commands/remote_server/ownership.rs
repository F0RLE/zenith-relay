use super::super::restart_or_rollback;
use super::{active_client, now_ms, object_path, remote_error};
use crate::local_pool::{
    accounts::{
        export_ops::mark_local_accounts_moved, exports::normalize_account_ids,
        import_orchestrator::stage_returned_remote_account,
        quota_refresh::prepare_preserved_remote_account_credentials,
    },
    error::{CommandError, ErrorCode, LocalPoolError},
    models::{
        LocalAccountRecord, OwnershipOperationKind, OwnershipOperationPhase,
        OwnershipOperationRecord,
    },
    profiles::codex,
    remote::{
        client::{RemoteClient, RemoteClientError},
        RemoteTargetRecord,
    },
    state::DesktopState,
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use tauri::{AppHandle, Emitter, State};
use zenith_relay_core::accounts::{AccountAuthState, AccountExportFormat, AccountExportRequest};
use zenith_relay_core::protocol::{Feature, RemoteAccountLocation, RuntimeStateSnapshot};

mod transfer;

use transfer::{delete_remote_accounts, transfer_local_account_batch};

const REMOTE_TRANSFER_VALIDATION_BATCH_SIZE: usize = 5;
const ACCOUNT_TRANSFER_PROGRESS_EVENT: &str = "relay-account-transfer-progress";
const REMOTE_MISSING_ERROR: &str = "remote_missing";
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountTransferProgressEvent {
    completed: usize,
    total: usize,
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_account_id: Option<String>,
}

pub(super) async fn move_local_accounts_to_remote(
    input: MoveLocalAccountsToRemoteInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<MoveLocalAccountsToRemoteResult, CommandError> {
    let account_ids = normalize_account_ids(input.account_ids)?;
    let _mutation = state.setup_guard().await;
    ensure_no_pending_ownership_operation(&state)?;
    ensure_local_accounts_transferable(&state, &account_ids)?;
    emit_account_transfer_progress(&app, 0, &account_ids, "preparing");
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
    let operation = new_move_operation(&target, account_ids);
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    let remote_account_ids =
        execute_move_operation(&state, &client, &target, operation, Some(&app)).await?;

    Ok(MoveLocalAccountsToRemoteResult {
        moved: remote_account_ids.len(),
        remote_account_ids,
    })
}

pub(super) async fn return_remote_account_to_local(
    input: ReturnRemoteAccountToLocalInput,
    state: State<'_, DesktopState>,
) -> Result<ReturnRemoteAccountToLocalResult, CommandError> {
    let local_account_id = normalize_account_ids(vec![input.local_account_id])?
        .pop()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "account id is required"))?;
    let _mutation = state.setup_guard().await;
    ensure_no_pending_ownership_operation(&state)?;
    let remote_location = state
        .store()?
        .account(&local_account_id)
        .and_then(|account| account.remote_location.clone())
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account is not managed by a server",
            )
        })?;
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
    let operation = new_return_operation(&target, local_account_id.clone(), remote_location);
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    execute_return_operation(&state, &client, operation).await?;
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
    let local_account_id = normalize_account_ids(vec![input.local_account_id])?
        .pop()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "account id is required"))?;
    let _mutation = state.setup_guard().await;
    ensure_no_pending_ownership_operation(&state)?;
    let remote_location = state
        .store()?
        .account(&local_account_id)
        .and_then(|account| account.remote_location.clone())
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::InvalidState,
                "account is not managed by a server",
            )
        })?;
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
    let operation = new_force_activation_operation(&remote_location, local_account_id.clone());
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    execute_force_activation(&state, operation).await?;
    Ok(ForceActivateRemoteAccountLocallyResult { local_account_id })
}

pub(super) fn ensure_no_pending_ownership_operation(
    state: &DesktopState,
) -> Result<(), LocalPoolError> {
    if state.store()?.ownership_operation().is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "a remote account ownership operation must recover before another can start",
        ));
    }
    Ok(())
}

fn new_move_operation(
    target: &RemoteTargetRecord,
    local_account_ids: Vec<String>,
) -> OwnershipOperationRecord {
    let now = now_ms();
    OwnershipOperationRecord {
        id: format!("ownership_{}", uuid::Uuid::new_v4().simple()),
        kind: OwnershipOperationKind::MoveToRemote,
        phase: OwnershipOperationPhase::MovePrepared,
        server_id: target.server_id.clone(),
        local_account_ids,
        remote_account_ids: Vec::new(),
        created_remote_account_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    }
}

fn new_return_operation(
    target: &RemoteTargetRecord,
    local_account_id: String,
    remote_location: RemoteAccountLocation,
) -> OwnershipOperationRecord {
    let now = now_ms();
    OwnershipOperationRecord {
        id: format!("ownership_{}", uuid::Uuid::new_v4().simple()),
        kind: OwnershipOperationKind::ReturnToLocal,
        phase: OwnershipOperationPhase::ReturnPrepared,
        server_id: target.server_id.clone(),
        local_account_ids: vec![local_account_id],
        remote_account_ids: vec![remote_location.remote_account_id],
        created_remote_account_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    }
}

fn new_force_activation_operation(
    remote_location: &RemoteAccountLocation,
    local_account_id: String,
) -> OwnershipOperationRecord {
    let now = now_ms();
    OwnershipOperationRecord {
        id: format!("ownership_{}", uuid::Uuid::new_v4().simple()),
        kind: OwnershipOperationKind::ForceActivateLocal,
        phase: OwnershipOperationPhase::ForcePrepared,
        server_id: remote_location.server_id.clone(),
        local_account_ids: vec![local_account_id],
        remote_account_ids: vec![remote_location.remote_account_id.clone()],
        created_remote_account_ids: Vec::new(),
        created_at_ms: now,
        updated_at_ms: now,
    }
}

async fn execute_move_operation(
    state: &DesktopState,
    client: &RemoteClient,
    target: &RemoteTargetRecord,
    mut operation: OwnershipOperationRecord,
    app: Option<&AppHandle>,
) -> Result<Vec<String>, LocalPoolError> {
    let account_ids = operation.local_account_ids.clone();
    let mut remote_account_ids = Vec::with_capacity(account_ids.len());
    let mut created_remote_account_ids = operation.created_remote_account_ids.clone();
    let mut completed = 0;
    for batch in account_ids.chunks(REMOTE_TRANSFER_VALIDATION_BATCH_SIZE) {
        if let Some(app) = app {
            emit_account_transfer_progress(app, completed, &account_ids, "transferring");
        }
        match transfer_local_account_batch(state, client, batch).await {
            Ok(transferred) => {
                remote_account_ids.extend(transferred.account_ids);
                extend_unique(
                    &mut created_remote_account_ids,
                    transferred.created_account_ids,
                );
                operation.phase = OwnershipOperationPhase::MoveRemoteApplying;
                operation.remote_account_ids = remote_account_ids.clone();
                operation.created_remote_account_ids = created_remote_account_ids.clone();
                operation.updated_at_ms = now_ms();
                state
                    .store()?
                    .replace_ownership_operation(Some(operation.clone()))?;
                for _ in batch {
                    completed += 1;
                    if let Some(app) = app {
                        emit_account_transfer_progress(
                            app,
                            completed,
                            &account_ids,
                            "transferring",
                        );
                    }
                }
            }
            Err(error) => {
                extend_unique(&mut created_remote_account_ids, error.created_account_ids);
                operation.phase = OwnershipOperationPhase::MoveRemoteApplying;
                operation.remote_account_ids = remote_account_ids;
                operation.created_remote_account_ids = created_remote_account_ids.clone();
                operation.updated_at_ms = now_ms();
                state
                    .store()?
                    .replace_ownership_operation(Some(operation))?;
                let rollback_complete =
                    delete_remote_accounts(client, &created_remote_account_ids).await;
                if rollback_complete && error.code != ErrorCode::RecoveryRequired {
                    state.store()?.replace_ownership_operation(None)?;
                    return Err(LocalPoolError::new(error.code, error.message));
                }
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    format!("{}; remote ownership recovery is required", error.message),
                ));
            }
        }
    }

    operation.phase = OwnershipOperationPhase::MoveRemoteCommitted;
    operation.remote_account_ids = remote_account_ids.clone();
    operation.created_remote_account_ids = created_remote_account_ids.clone();
    operation.updated_at_ms = now_ms();
    state
        .store()?
        .replace_ownership_operation(Some(operation.clone()))?;
    let remote_locations = move_remote_locations(target, &operation)?;
    if let Some(app) = app {
        emit_account_transfer_progress(app, completed, &account_ids, "committing");
    }
    if let Err(error) =
        deactivate_transferred_local_accounts(state, &remote_locations, &operation).await
    {
        let rollback_complete = delete_remote_accounts(client, &created_remote_account_ids).await;
        if rollback_complete {
            state.store()?.replace_ownership_operation(None)?;
        }
        let message = if rollback_complete {
            format!("remote import was rolled back after local deactivation failed: {error}")
        } else {
            format!("local deactivation failed and remote recovery is incomplete: {error}")
        };
        return Err(LocalPoolError::new(ErrorCode::RecoveryRequired, message));
    }
    state.store()?.replace_ownership_operation(None)?;
    if let Some(app) = app {
        emit_account_transfer_progress(app, completed, &account_ids, "complete");
    }
    Ok(remote_account_ids)
}

async fn execute_return_operation(
    state: &DesktopState,
    client: &RemoteClient,
    mut operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let local_account_id = operation
        .local_account_ids
        .first()
        .cloned()
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "return operation has no local account",
            )
        })?;
    let remote_account_id = operation
        .remote_account_ids
        .first()
        .cloned()
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "return operation has no remote account",
            )
        })?;
    if operation.local_account_ids.len() != 1 || operation.remote_account_ids.len() != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "return operation must contain one account",
        ));
    }

    if operation.phase == OwnershipOperationPhase::ReturnLocalCommitted {
        if !local_return_is_committed(state, &local_account_id)? {
            activate_returned_local_account(state, &local_account_id, &operation).await?;
        }
        state.store()?.replace_ownership_operation(None)?;
        return Ok(());
    }

    if operation.phase == OwnershipOperationPhase::ReturnPrepared {
        let document = client
            .export_accounts(&AccountExportRequest {
                account_ids: vec![remote_account_id.clone()],
                format: AccountExportFormat::Zenith,
                description: None,
            })
            .await
            .map_err(|error| {
                LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string())
            })?;
        stage_returned_remote_account(state, &local_account_id, &document.content).await?;
        operation.phase = OwnershipOperationPhase::ReturnLocalStaged;
        operation.updated_at_ms = now_ms();
        state
            .store()?
            .replace_ownership_operation(Some(operation.clone()))?;
    }

    if operation.phase == OwnershipOperationPhase::ReturnLocalStaged {
        ensure_local_ownership_is_staged(state, &local_account_id, &operation)?;
        remove_remote_account_for_return(client, &remote_account_id).await?;
        operation.phase = OwnershipOperationPhase::ReturnRemoteRemoved;
        operation.updated_at_ms = now_ms();
        state
            .store()?
            .replace_ownership_operation(Some(operation.clone()))?;
    }

    if operation.phase == OwnershipOperationPhase::ReturnRemoteRemoved {
        activate_returned_local_account(state, &local_account_id, &operation).await?;
    }
    state.store()?.replace_ownership_operation(None)?;
    Ok(())
}

async fn execute_force_activation(
    state: &DesktopState,
    mut operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let local_account_id = operation
        .local_account_ids
        .first()
        .cloned()
        .ok_or_else(|| {
            LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                "forced recovery operation has no local account",
            )
        })?;
    if operation.local_account_ids.len() != 1 || operation.remote_account_ids.len() != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "forced recovery operation must contain one account",
        ));
    }
    if operation.phase == OwnershipOperationPhase::ForceLocalCommitted {
        if !local_return_is_committed(state, &local_account_id)? {
            ensure_local_ownership_is_staged(state, &local_account_id, &operation)?;
            activate_local_account_with_operation(state, &local_account_id, operation.clone())
                .await?;
        }
        state.store()?.replace_ownership_operation(None)?;
        return Ok(());
    }
    ensure_local_ownership_is_staged(state, &local_account_id, &operation)?;
    prepare_preserved_remote_account_credentials(state, &local_account_id).await?;
    operation.phase = OwnershipOperationPhase::ForceLocalCommitted;
    operation.updated_at_ms = now_ms();
    activate_local_account_with_operation(state, &local_account_id, operation).await?;
    state.store()?.replace_ownership_operation(None)?;
    Ok(())
}

fn ensure_local_ownership_is_staged(
    state: &DesktopState,
    local_account_id: &str,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let account = state
        .store()?
        .account(local_account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local account not found"))?;
    if account.account.enabled
        || account.account.in_pool
        || account.remote_location.as_ref().is_none_or(|location| {
            location.server_id != operation.server_id
                || operation.remote_account_ids.first() != Some(&location.remote_account_id)
        })
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "account recovery credentials are not staged on the inactive local record",
        ));
    }
    Ok(())
}

async fn remove_remote_account_for_return(
    client: &RemoteClient,
    remote_account_id: &str,
) -> Result<(), LocalPoolError> {
    if !remote_account_exists(client, remote_account_id).await? {
        return Ok(());
    }
    let path = object_path("accounts", remote_account_id)
        .map_err(|error| LocalPoolError::new(error.code, error.message))?;
    match client.mutate(Method::DELETE, &path, None).await {
        Ok(_) | Err(RemoteClientError::HttpStatus(404)) => {}
        Err(error) => {
            return Err(LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                error.to_string(),
            ));
        }
    }
    if remote_account_exists(client, remote_account_id).await? {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "remote server still reports the account after deletion",
        ));
    }
    Ok(())
}

async fn remote_account_exists(
    client: &RemoteClient,
    remote_account_id: &str,
) -> Result<bool, LocalPoolError> {
    client
        .state()
        .await
        .map(|snapshot| {
            snapshot
                .accounts
                .iter()
                .any(|account| account.id == remote_account_id)
        })
        .map_err(|error| LocalPoolError::new(ErrorCode::GatewayUnavailable, error.to_string()))
}

async fn activate_returned_local_account(
    state: &DesktopState,
    local_account_id: &str,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let mut committed = operation.clone();
    committed.phase = OwnershipOperationPhase::ReturnLocalCommitted;
    committed.updated_at_ms = now_ms();
    activate_local_account_with_operation(state, local_account_id, committed).await
}

/// Commits a local ownership transition and restores both storage and the
/// previous gateway runtime if the replacement runtime cannot start.
async fn commit_local_ownership_change(
    state: &DesktopState,
    accounts: Vec<LocalAccountRecord>,
    operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let (old_accounts, old_keys, old_operation) = {
        let store = state.store()?;
        (
            store.accounts().to_vec(),
            store.keys().to_vec(),
            store.ownership_operation().cloned(),
        )
    };
    state
        .store()?
        .replace_accounts_keys_and_ownership_operation(
            accounts,
            old_keys.clone(),
            Some(operation),
        )?;
    restart_or_rollback(state, || {
        state
            .store()?
            .replace_accounts_keys_and_ownership_operation(old_accounts, old_keys, old_operation)
    })
    .await
}

async fn activate_local_account_with_operation(
    state: &DesktopState,
    local_account_id: &str,
    committed_operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let mut accounts = state.store()?.accounts().to_vec();
    let account = accounts
        .iter_mut()
        .find(|account| account.account.id == local_account_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local account not found"))?;
    account.remote_location = None;
    account.account.enabled = true;
    account.account.in_pool = true;
    account.account.draining = false;
    if account.account.last_error_code.as_deref() == Some(REMOTE_MISSING_ERROR) {
        account.account.last_error_code = None;
    }
    commit_local_ownership_change(state, accounts, committed_operation).await?;
    let _ = state.sync_account_quota_refresh(local_account_id, now_ms());
    Ok(())
}

fn local_return_is_committed(
    state: &DesktopState,
    local_account_id: &str,
) -> Result<bool, LocalPoolError> {
    Ok(state
        .store()?
        .account(local_account_id)
        .is_some_and(|account| {
            account.remote_location.is_none() && account.account.enabled && account.account.in_pool
        }))
}

fn move_remote_locations(
    target: &RemoteTargetRecord,
    operation: &OwnershipOperationRecord,
) -> Result<HashMap<String, RemoteAccountLocation>, LocalPoolError> {
    if operation.local_account_ids.len() != operation.remote_account_ids.len() {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "remote ownership operation does not contain every account mapping",
        ));
    }
    Ok(operation
        .local_account_ids
        .iter()
        .cloned()
        .zip(
            operation
                .remote_account_ids
                .iter()
                .cloned()
                .map(|remote_account_id| RemoteAccountLocation {
                    server_id: target.server_id.clone(),
                    remote_account_id,
                }),
        )
        .collect())
}

fn extend_unique(target: &mut Vec<String>, values: impl IntoIterator<Item = String>) {
    for value in values {
        if !target.contains(&value) {
            target.push(value);
        }
    }
}

pub(crate) async fn recover_pending_remote_ownership(
    state: &DesktopState,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let Some(operation) = state.store()?.ownership_operation().cloned() else {
        return Ok(());
    };
    if operation.kind == OwnershipOperationKind::ForceActivateLocal {
        execute_force_activation(state, operation).await?;
        return Ok(());
    }
    let Some((target, client)) = active_client(state)? else {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "reconnect the recorded server to recover account ownership",
        )
        .into());
    };
    if target.server_id != operation.server_id {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "the connected server does not own the pending account operation",
        )
        .into());
    }
    match operation.kind {
        OwnershipOperationKind::MoveToRemote => {
            if operation.phase == OwnershipOperationPhase::MoveLocalCommitted {
                if !local_move_is_committed(state, &operation)? {
                    let locations = move_remote_locations(&target, &operation)?;
                    deactivate_transferred_local_accounts(state, &locations, &operation).await?;
                }
                state.store()?.replace_ownership_operation(None)?;
                return Ok(());
            }
            ensure_move_accounts_still_present(state, &operation)?;
            let capabilities = client.capabilities().await.map_err(remote_error)?;
            if !capabilities.supports(Feature::AccountBatchImport)
                || !capabilities.supports(Feature::AccountBatchImportCreationStatus)
            {
                return Err(LocalPoolError::new(
                    ErrorCode::RecoveryRequired,
                    "the connected server cannot safely recover the pending account move",
                )
                .into());
            }
            execute_move_operation(state, &client, &target, operation, None).await?;
        }
        OwnershipOperationKind::ReturnToLocal => {
            if matches!(
                operation.phase,
                OwnershipOperationPhase::ReturnPrepared
                    | OwnershipOperationPhase::ReturnLocalStaged
            ) {
                let capabilities = client.capabilities().await.map_err(remote_error)?;
                if !capabilities.supports(Feature::AccountExport) {
                    return Err(LocalPoolError::new(
                        ErrorCode::RecoveryRequired,
                        "the connected server cannot recover the pending account return",
                    )
                    .into());
                }
            }
            execute_return_operation(state, &client, operation).await?;
        }
        OwnershipOperationKind::ForceActivateLocal => unreachable!(),
    }
    Ok(())
}

pub(crate) async fn reconcile_saved_remote_ownership(
    state: &DesktopState,
) -> Result<(), CommandError> {
    let _mutation = state.setup_guard().await;
    let (has_pending_operation, has_linked_accounts) = {
        let store = state.store()?;
        let Some(target) = store.remote_target() else {
            return Ok(());
        };
        (
            store.ownership_operation().is_some(),
            store.accounts().iter().any(|account| {
                account
                    .remote_location
                    .as_ref()
                    .is_some_and(|location| location.server_id == target.server_id)
            }),
        )
    };
    if has_pending_operation || !has_linked_accounts {
        return Ok(());
    }
    let Some((target, client)) = active_client(state)? else {
        return Ok(());
    };
    let snapshot = client.state().await.map_err(remote_error)?;
    reconcile_remote_account_locations(state, &target, &snapshot)?;
    Ok(())
}

pub(super) fn reconcile_remote_account_locations(
    state: &DesktopState,
    target: &RemoteTargetRecord,
    snapshot: &RuntimeStateSnapshot,
) -> Result<(), LocalPoolError> {
    let remote_ids = snapshot
        .accounts
        .iter()
        .map(|account| account.id.as_str())
        .collect::<HashSet<_>>();
    let (mut accounts, keys) = {
        let store = state.store()?;
        (store.accounts().to_vec(), store.keys().to_vec())
    };
    let mut changed = false;
    for account in &mut accounts {
        let Some(location) = account
            .remote_location
            .as_ref()
            .filter(|location| location.server_id == target.server_id)
        else {
            continue;
        };
        let next_error = reconciled_remote_error(
            account.account.last_error_code.as_deref(),
            remote_ids.contains(location.remote_account_id.as_str()),
        );
        if account.account.last_error_code != next_error {
            account.account.last_error_code = next_error;
            changed = true;
        }
        if account.account.enabled || account.account.in_pool {
            account.account.enabled = false;
            account.account.in_pool = false;
            changed = true;
        }
    }
    if changed {
        state.store()?.replace_accounts_and_keys(accounts, keys)?;
    }
    Ok(())
}

fn reconciled_remote_error(current: Option<&str>, remote_exists: bool) -> Option<String> {
    if remote_exists {
        current
            .filter(|code| *code != REMOTE_MISSING_ERROR)
            .map(str::to_string)
    } else {
        Some(REMOTE_MISSING_ERROR.to_string())
    }
}

fn ensure_move_accounts_still_present(
    state: &DesktopState,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let store = state.store()?;
    if operation.local_account_ids.iter().any(|account_id| {
        store.account(account_id).is_none_or(|account| {
            account.remote_location.as_ref().is_some_and(|location| {
                location.server_id != operation.server_id
                    || !operation
                        .remote_account_ids
                        .contains(&location.remote_account_id)
            })
        })
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "a local account changed while its remote move was incomplete",
        ));
    }
    Ok(())
}

fn local_move_is_committed(
    state: &DesktopState,
    operation: &OwnershipOperationRecord,
) -> Result<bool, LocalPoolError> {
    if operation.local_account_ids.len() != operation.remote_account_ids.len() {
        return Ok(false);
    }
    let store = state.store()?;
    Ok(operation
        .local_account_ids
        .iter()
        .zip(&operation.remote_account_ids)
        .all(|(local_id, remote_id)| {
            store.account(local_id).is_some_and(|account| {
                !account.account.enabled
                    && !account.account.in_pool
                    && account.remote_location.as_ref().is_some_and(|location| {
                        location.server_id == operation.server_id
                            && location.remote_account_id == *remote_id
                    })
            })
        }))
}

fn ensure_local_accounts_transferable(
    state: &DesktopState,
    account_ids: &[String],
) -> Result<(), LocalPoolError> {
    {
        let store = state.store()?;
        for account_id in account_ids {
            let account = store.account(account_id).ok_or_else(|| {
                LocalPoolError::new(
                    ErrorCode::NotFound,
                    "an account selected for transfer was not found",
                )
            })?;
            if !account_auth_can_transfer_to_remote(account.account.auth_state) {
                return Err(LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "access-only accounts must sign in again before server transfer",
                ));
            }
            if account.remote_location.is_some() {
                return Err(LocalPoolError::new(
                    ErrorCode::Conflict,
                    "account is already managed by a remote server",
                ));
            }
        }
    }
    let bindings = codex::profile_bindings(
        &crate::platform::default_codex_home(),
        &state.profile_backup_root(),
    )?;
    if bindings.iter().any(|binding| {
        binding.active
            && (account_ids.contains(&binding.credential_id)
                || binding
                    .bound_oauth_account_id
                    .as_ref()
                    .is_some_and(|id| account_ids.contains(id)))
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "restore the direct ChatGPT profile or attach it to the remote gateway before moving this account",
        ));
    }
    Ok(())
}

fn account_auth_can_transfer_to_remote(auth_state: AccountAuthState) -> bool {
    auth_state != AccountAuthState::DegradedAccessOnly
}

fn emit_account_transfer_progress(
    app: &AppHandle,
    completed: usize,
    account_ids: &[String],
    phase: &'static str,
) {
    let _ = app.emit(
        ACCOUNT_TRANSFER_PROGRESS_EVENT,
        AccountTransferProgressEvent {
            completed,
            total: account_ids.len(),
            phase,
            current_account_id: account_ids.get(completed).cloned(),
        },
    );
}

async fn deactivate_transferred_local_accounts(
    state: &DesktopState,
    remote_locations: &HashMap<String, RemoteAccountLocation>,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let mut accounts = state.store()?.accounts().to_vec();
    mark_local_accounts_moved(&mut accounts, remote_locations)?;
    let mut committed = operation.clone();
    committed.phase = OwnershipOperationPhase::MoveLocalCommitted;
    committed.updated_at_ms = now_ms();
    commit_local_ownership_change(state, accounts, committed).await?;
    for account_id in remote_locations.keys() {
        let _ = state.sync_account_quota_refresh(account_id, now_ms());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_only_account_cannot_start_server_transfer() {
        assert!(account_auth_can_transfer_to_remote(
            AccountAuthState::Active
        ));
        assert!(!account_auth_can_transfer_to_remote(
            AccountAuthState::DegradedAccessOnly
        ));
    }

    #[test]
    fn remote_reconciliation_is_fail_closed_and_clears_only_its_own_error() {
        assert_eq!(
            reconciled_remote_error(None, false).as_deref(),
            Some(REMOTE_MISSING_ERROR)
        );
        assert_eq!(
            reconciled_remote_error(Some(REMOTE_MISSING_ERROR), true),
            None
        );
        assert_eq!(
            reconciled_remote_error(Some("token_invalidated"), true).as_deref(),
            Some("token_invalidated")
        );
    }

    #[test]
    fn forced_local_recovery_is_a_valid_persisted_ownership_operation() {
        let operation = new_force_activation_operation(
            &RemoteAccountLocation {
                server_id: "server-one".into(),
                remote_account_id: "account-remote".into(),
            },
            "account-local".into(),
        );

        assert!(operation.validate().is_ok());
        assert_eq!(operation.kind, OwnershipOperationKind::ForceActivateLocal);
        assert_eq!(operation.phase, OwnershipOperationPhase::ForcePrepared);
    }
}
