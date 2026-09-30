use super::super::super::fence_runtime_candidates;
use super::super::now_ms;
use crate::local_pool::{
    accounts::{
        import_orchestrator::stage_returned_remote_account,
        quota_refresh::prepare_preserved_remote_account_credentials,
    },
    error::{ErrorCode, LocalPoolError},
    models::{OwnershipOperationKind, OwnershipOperationPhase, OwnershipOperationRecord},
    remote::{client::RemoteClient, RemoteTargetRecord},
    state::DesktopState,
};
use tauri::AppHandle;
use zenith_relay_core::accounts::{AccountExportFormat, AccountExportRequest};

use super::transfer::{delete_remote_accounts, transfer_local_account_batch};
use zenith_relay_core::protocol::RemoteAccountLocation;

const REMOTE_TRANSFER_VALIDATION_BATCH_SIZE: usize = 5;

mod steps;

#[cfg(test)]
pub(super) use steps::{account_auth_can_transfer_to_remote, commit_local_ownership_change};
pub(super) use steps::{
    activate_local_account_with_operation, activate_returned_local_account,
    deactivate_transferred_local_accounts, emit_account_transfer_progress,
    ensure_local_accounts_transferable, ensure_local_ownership_is_staged, extend_unique,
    local_move_is_committed, local_return_is_committed, move_remote_locations,
    persist_remote_move_operation, remote_accounts_absent, remove_remote_account_for_return,
};

pub(in crate::local_pool::commands::remote_server) fn ensure_no_pending_ownership_operation(
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

pub(super) fn new_move_operation(
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

pub(super) fn new_return_operation(
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

pub(super) fn new_force_activation_operation(
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

pub(super) async fn execute_move_operation(
    state: &DesktopState,
    client: &RemoteClient,
    target: &RemoteTargetRecord,
    mut operation: OwnershipOperationRecord,
    app: Option<&AppHandle>,
) -> Result<Vec<String>, LocalPoolError> {
    let account_ids = operation.local_account_ids.clone();
    // The remote can accept an import before the local ownership commit. Keep
    // pending local dispatch closed until success or verified remote cleanup.
    let runtime = state.gateway.runtime().await;
    let _transfer_fences = fence_runtime_candidates(runtime.as_deref(), &account_ids, &[]);
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
                persist_remote_move_operation(state, operation.clone()).await?;
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
                let mut known_remote_ids = operation.remote_account_ids.clone();
                extend_unique(&mut known_remote_ids, created_remote_account_ids.clone());
                persist_remote_move_operation(state, operation).await?;
                let rollback_complete = delete_remote_accounts(client, &created_remote_account_ids)
                    .await
                    && remote_accounts_absent(client, &known_remote_ids).await;
                if rollback_complete && error.code != ErrorCode::RecoveryRequired {
                    state.store()?.replace_ownership_operation(None)?;
                    return Err(LocalPoolError::new(error.code, error.message));
                }
                return Err(super::super::super::fail_closed(
                    state,
                    format!("{}; remote ownership recovery is required", error.message),
                )
                .await);
            }
        }
    }

    operation.phase = OwnershipOperationPhase::MoveRemoteCommitted;
    operation.remote_account_ids = remote_account_ids.clone();
    operation.created_remote_account_ids = created_remote_account_ids.clone();
    operation.updated_at_ms = now_ms();
    persist_remote_move_operation(state, operation.clone()).await?;
    let remote_locations = match move_remote_locations(target, &operation) {
        Ok(locations) => locations,
        Err(error) => {
            return Err(super::super::super::fail_closed(state, error.to_string()).await);
        }
    };
    if let Some(app) = app {
        emit_account_transfer_progress(app, completed, &account_ids, "committing");
    }
    if let Err(error) =
        deactivate_transferred_local_accounts(state, &remote_locations, &operation).await
    {
        let locally_committed = match local_move_is_committed(state, &operation) {
            Ok(committed) => committed,
            Err(inspect) => {
                return Err(super::super::super::fail_closed(
                    state,
                    format!("local move inspection failed: {inspect}"),
                )
                .await);
            }
        };
        if locally_committed {
            // The safe local disable was saved. Keep the remote copy and the
            // recovery marker; never roll ownership back merely because the
            // replacement listener failed to start.
            return Err(LocalPoolError::new(
                ErrorCode::RecoveryRequired,
                format!("local runtime replacement needs recovery: {error}"),
            ));
        }
        let rollback_complete = delete_remote_accounts(client, &created_remote_account_ids).await
            && remote_accounts_absent(client, &remote_account_ids).await;
        if rollback_complete && error.code != ErrorCode::RecoveryRequired {
            state.store()?.replace_ownership_operation(None)?;
        } else {
            return Err(super::super::super::fail_closed(
                state,
                format!("local deactivation failed and remote recovery is incomplete: {error}"),
            )
            .await);
        }
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            format!("remote import was rolled back after local deactivation failed: {error}"),
        ));
    }
    state.store()?.replace_ownership_operation(None)?;
    if let Some(app) = app {
        emit_account_transfer_progress(app, completed, &account_ids, "complete");
    }
    Ok(remote_account_ids)
}

pub(super) async fn execute_return_operation(
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

pub(super) async fn execute_force_activation(
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
