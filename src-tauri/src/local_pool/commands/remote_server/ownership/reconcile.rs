use super::super::super::{
    apply_local_gateway_key_scope, fence_runtime_candidates, restart_or_rollback,
    runtime_account_policy,
};
use super::super::{active_client, now_ms, remote_error};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    models::{OwnershipOperationKind, OwnershipOperationPhase, OwnershipOperationRecord},
    remote::RemoteTargetRecord,
    state::DesktopState,
};

use std::collections::HashSet;

use super::execution::{
    deactivate_transferred_local_accounts, execute_force_activation, execute_move_operation,
    execute_return_operation, local_move_is_committed, move_remote_locations,
};
use super::REMOTE_MISSING_ERROR;
use zenith_relay_core::protocol::{Feature, RuntimeStateSnapshot};

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
                    let runtime = state.gateway.runtime().await;
                    let _fences = fence_runtime_candidates(
                        runtime.as_deref(),
                        &operation.local_account_ids,
                        &[],
                    );
                    if let Err(error) =
                        deactivate_transferred_local_accounts(state, &locations, &operation).await
                    {
                        return Err(super::super::super::fail_closed(state, error.to_string())
                            .await
                            .into());
                    }
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
    reconcile_remote_account_locations(state, &target, &snapshot).await?;
    Ok(())
}

pub(in crate::local_pool::commands::remote_server) async fn reconcile_remote_account_locations(
    state: &DesktopState,
    target: &RemoteTargetRecord,
    snapshot: &RuntimeStateSnapshot,
) -> Result<(), LocalPoolError> {
    let remote_ids = snapshot
        .accounts
        .iter()
        .map(|account| account.id.as_str())
        .collect::<HashSet<_>>();
    reconcile_remote_account_ids(state, target, &remote_ids).await
}

pub(super) async fn reconcile_remote_account_ids(
    state: &DesktopState,
    target: &RemoteTargetRecord,
    remote_ids: &HashSet<&str>,
) -> Result<(), LocalPoolError> {
    let (mut accounts, keys) = {
        let store = state.store()?;
        (store.accounts().to_vec(), store.keys().to_vec())
    };
    let mut changed = false;
    let mut affected_ids = Vec::new();
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
            affected_ids.push(account.account.id.clone());
            changed = true;
        }
    }
    if !changed {
        return Ok(());
    }
    // A remote-owned account must not keep a pending local dispatch between
    // the durable reconciliation and the live policy/scope replacement.
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &affected_ids, &[]);
    state
        .store()?
        .replace_accounts_and_keys(accounts.clone(), keys.clone())?;
    if !affected_ids.is_empty() {
        if let Some(runtime) = runtime {
            let now_ms = now_ms();
            let applied = accounts
                .iter()
                .filter(|account| affected_ids.contains(&account.account.id))
                .all(|account| {
                    runtime.update_account_policy(
                        &account.account.id,
                        runtime_account_policy(account, now_ms),
                    )
                })
                && apply_local_gateway_key_scope(state, &runtime).unwrap_or(false);
            if !applied {
                // The old persisted state already had an erroneously live
                // remote-owned account. Never roll back to that unsafe state.
                restart_or_rollback(state, || Ok(())).await?;
            }
        }
    }
    Ok(())
}

pub(super) fn reconciled_remote_error(
    stored_error_code: Option<&str>,
    remote_exists: bool,
) -> Option<String> {
    if remote_exists {
        stored_error_code
            .filter(|code| *code != REMOTE_MISSING_ERROR)
            .map(str::to_string)
    } else {
        Some(REMOTE_MISSING_ERROR.to_string())
    }
}

pub(super) fn ensure_move_accounts_still_present(
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
