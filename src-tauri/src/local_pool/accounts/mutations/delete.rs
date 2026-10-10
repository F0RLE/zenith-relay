use super::super::authority::{
    ProcessAccountGuard, ProcessAccountLocks, ProcessLockConfig, ProcessLockError,
};
use super::super::import_orchestrator::credential_local_error;
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::exports::normalize_account_ids;
use crate::local_pool::accounts::proxy::ProxyPool;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::{
    fail_closed, fence_runtime_candidates, refresh_active_codex_catalog_in_background,
};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::{
    AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot,
};
use crate::local_pool::profiles::codex;
use crate::local_pool::state::DesktopState;
use tauri::{AppHandle, State};

type CommandResult<T> = std::result::Result<T, CommandError>;

mod cleanup;
mod prepare;
pub(in crate::local_pool::accounts) use prepare::ensure_accounts_exist;
use prepare::{
    acquire_delete_credential_guards, delete_local_account_inner,
    ensure_accounts_not_in_ownership_operation, ensure_delete_rollback_or_fail_closed,
    prepare_delete_local_account, rollback_batch_delete,
};
#[cfg(test)]
use prepare::{rollback_prepared_delete, PreparedAccountDelete};

pub(in crate::local_pool::accounts) use cleanup::{
    prune_account_task_selectors, restore_bound_account_profiles,
    rollback_deleted_account_side_effects,
};

#[tauri::command]
pub async fn delete_local_account(
    account_id: String,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let _mutation = state.setup_guard().await;
    ensure_accounts_not_in_ownership_operation(&state, std::slice::from_ref(&account_id))?;
    delete_local_account_inner(&account_id, &state).await?;
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    refresh_active_codex_catalog_in_background(app);
    Ok(snapshot)
}

#[tauri::command]
pub async fn delete_local_accounts(
    account_ids: Vec<String>,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<LocalPoolSnapshot> {
    let account_ids = normalize_account_ids(account_ids)?;
    let _mutation = state.setup_guard().await;
    ensure_accounts_not_in_ownership_operation(&state, &account_ids)?;
    let existing_accounts = state.store()?.accounts().to_vec();
    ensure_accounts_exist(&existing_accounts, &account_ids)?;
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &account_ids, &[]);
    let account_id_refs = account_ids.iter().map(String::as_str).collect::<Vec<_>>();
    let _credential_guards = acquire_delete_credential_guards(&state, &account_id_refs).await?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let initial_wake = state.wake_snapshot()?;
    let initial_automations = state.store()?.automations().clone();
    let initial_proxy_pool = ProxyPool::load()?;
    let old_keys = state.store()?.keys().to_vec();
    let mut automations = initial_automations.clone();
    for account_id in &account_ids {
        automations = prune_account_task_selectors(automations, account_id);
    }
    let mut prepared = Vec::with_capacity(account_ids.len());
    for account_id in &account_ids {
        match prepare_delete_local_account(account_id, &state, &credentials).await {
            Ok(deleted) => prepared.push(deleted),
            Err(error) => {
                ensure_delete_rollback_or_fail_closed(
                    &state,
                    rollback_batch_delete(
                        &state,
                        &credentials,
                        &prepared,
                        initial_wake,
                        initial_automations,
                        &initial_proxy_pool,
                        &error,
                    ),
                )
                .await?;
                return Err(error.into());
            }
        }
    }
    let accounts = existing_accounts
        .iter()
        .filter(|account| !account_ids.contains(&account.account.id))
        .cloned()
        .collect::<Vec<_>>();
    let delete_result = state.store().and_then(|mut store| {
        store.delete_accounts_state(&account_ids, accounts, old_keys, automations)
    });
    if let Err(error) = delete_result {
        ensure_delete_rollback_or_fail_closed(
            &state,
            rollback_batch_delete(
                &state,
                &credentials,
                &prepared,
                initial_wake,
                initial_automations,
                &initial_proxy_pool,
                &error,
            ),
        )
        .await?;
        return Err(error.into());
    }
    if let Some(runtime) = state.gateway.runtime().await {
        for account_id in &account_ids {
            runtime.remove_candidate(account_id);
        }
    }
    for account_id in &account_ids {
        state.token_authority().remove(account_id);
        let _ = state.remove_quota_account_lock(account_id);
    }
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    refresh_active_codex_catalog_in_background(app);
    Ok(snapshot)
}

#[cfg(test)]
mod tests;
