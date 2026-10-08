use super::super::super::super::{fence_runtime_candidates, restart_or_rollback};
use super::super::super::{now_ms, object_path};
use super::super::REMOTE_MISSING_ERROR;
use crate::local_pool::{
    accounts::export_ops::mark_local_accounts_moved,
    error::{ErrorCode, LocalPoolError},
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
use serde::Serialize;
use std::collections::HashMap;
use tauri::{AppHandle, Emitter};
use zenith_relay_core::accounts::AccountAuthState;
use zenith_relay_core::protocol::RemoteAccountLocation;

const ACCOUNT_TRANSFER_PROGRESS_EVENT: &str = "relay-account-transfer-progress";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AccountTransferProgressEvent {
    completed: usize,
    total: usize,
    phase: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    current_account_id: Option<String>,
}

pub(in crate::local_pool::commands::remote_server::ownership) fn ensure_local_ownership_is_staged(
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

pub(in crate::local_pool::commands::remote_server::ownership) async fn remove_remote_account_for_return(
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

pub(in crate::local_pool::commands::remote_server::ownership) async fn remote_account_exists(
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

pub(in crate::local_pool::commands::remote_server::ownership) async fn remote_accounts_absent(
    client: &RemoteClient,
    account_ids: &[String],
) -> bool {
    client.state().await.ok().is_some_and(|snapshot| {
        !snapshot
            .accounts
            .iter()
            .any(|account| account_ids.contains(&account.id))
    })
}

pub(in crate::local_pool::commands::remote_server::ownership) async fn persist_remote_move_operation(
    state: &DesktopState,
    operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    if let Err(error) = state
        .store()
        .and_then(|mut store| store.replace_ownership_operation(Some(operation)))
    {
        return Err(super::super::super::super::fail_closed(
            state,
            format!("failed to save remote ownership recovery: {error}"),
        )
        .await);
    }
    Ok(())
}

pub(in crate::local_pool::commands::remote_server::ownership) async fn activate_returned_local_account(
    state: &DesktopState,
    local_account_id: &str,
    operation: &OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let mut committed = operation.clone();
    committed.phase = OwnershipOperationPhase::ReturnLocalCommitted;
    committed.updated_at_ms = now_ms();
    activate_local_account_with_operation(state, local_account_id, committed).await
}

/// Commits a local ownership transition. Return/force activation restores the
/// previous inactive record on failure; a committed move keeps the local route
/// closed because the remote may already be serving that account.
pub(in crate::local_pool::commands::remote_server::ownership) async fn commit_local_ownership_change(
    state: &DesktopState,
    accounts: Vec<LocalAccountRecord>,
    operation: OwnershipOperationRecord,
) -> Result<(), LocalPoolError> {
    let moving_to_remote = operation.kind == OwnershipOperationKind::MoveToRemote;
    let (old_accounts, old_keys, old_operation) = {
        let store = state.store()?;
        (
            store.accounts().to_vec(),
            store.keys().to_vec(),
            store.ownership_operation().cloned(),
        )
    };
    let affected_ids = old_accounts
        .iter()
        .filter(|previous_account| {
            accounts
                .iter()
                .find(|account| account.account.id == previous_account.account.id)
                .is_none_or(|account| {
                    previous_account.remote_location != account.remote_location
                        || previous_account.account.enabled != account.account.enabled
                        || previous_account.account.in_pool != account.account.in_pool
                        || previous_account.account.draining != account.account.draining
                })
        })
        .map(|account| account.account.id.clone())
        .collect::<Vec<_>>();
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &affected_ids, &[]);
    state
        .store()?
        .replace_accounts_keys_and_ownership_operation(
            accounts,
            old_keys.clone(),
            Some(operation),
        )?;
    restart_or_rollback(state, || {
        if moving_to_remote {
            // The remote import has already committed. Re-enabling the local
            // candidate on a listener failure would create two owners.
            Ok(())
        } else {
            state
                .store()?
                .replace_accounts_keys_and_ownership_operation(
                    old_accounts,
                    old_keys,
                    old_operation,
                )
        }
    })
    .await
    .map_err(|error| {
        if moving_to_remote {
            LocalPoolError::new(ErrorCode::RecoveryRequired, error.to_string())
        } else {
            error
        }
    })
}

pub(in crate::local_pool::commands::remote_server::ownership) async fn activate_local_account_with_operation(
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

pub(in crate::local_pool::commands::remote_server::ownership) fn local_return_is_committed(
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

pub(in crate::local_pool::commands::remote_server::ownership) fn move_remote_locations(
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

pub(in crate::local_pool::commands::remote_server::ownership) fn extend_unique(
    target: &mut Vec<String>,
    account_ids: impl IntoIterator<Item = String>,
) {
    for account_id in account_ids {
        if !target.contains(&account_id) {
            target.push(account_id);
        }
    }
}

pub(in crate::local_pool::commands::remote_server::ownership) fn local_move_is_committed(
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

pub(in crate::local_pool::commands::remote_server::ownership) fn ensure_local_accounts_transferable(
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
                    .is_some_and(|account_id| account_ids.contains(account_id)))
    }) {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "restore the direct ChatGPT profile or attach it to the remote gateway before moving this account",
        ));
    }
    Ok(())
}

pub(in crate::local_pool::commands::remote_server::ownership) fn account_auth_can_transfer_to_remote(
    auth_state: AccountAuthState,
) -> bool {
    auth_state != AccountAuthState::DegradedAccessOnly
}

pub(in crate::local_pool::commands::remote_server::ownership) fn emit_account_transfer_progress(
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

pub(in crate::local_pool::commands::remote_server::ownership) async fn deactivate_transferred_local_accounts(
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
