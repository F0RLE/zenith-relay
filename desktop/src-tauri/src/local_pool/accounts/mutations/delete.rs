use super::super::authority::{
    ProcessAccountGuard, ProcessAccountLocks, ProcessLockConfig, ProcessLockError,
};
use super::super::import_orchestrator::credential_local_error;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::exports::normalize_account_ids;
use crate::local_pool::accounts::proxy::ProxyPool;
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::{
    AutomationRecords, LocalAccountRecord, LocalGatewayKeyRecord, LocalPoolSnapshot,
};
use crate::local_pool::profiles::codex;
use crate::local_pool::runtime::{
    fail_closed, fence_runtime_candidates, refresh_active_codex_catalog_in_background,
};
use crate::local_pool::state::DesktopState;
use tauri::{AppHandle, State};

type CommandResult<T> = std::result::Result<T, CommandError>;

mod cleanup;
mod prepare;
pub(crate) mod trash;
#[cfg(test)]
pub(in crate::local_pool::accounts) use prepare::PreparedAccountDelete;
pub(in crate::local_pool::accounts) use prepare::ensure_accounts_exist;
#[cfg(test)]
use prepare::rollback_prepared_delete;
use prepare::{
    acquire_delete_credential_guards, delete_local_account_inner,
    ensure_delete_rollback_or_fail_closed, prepare_delete_local_account, rollback_batch_delete,
};

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
    let accounts = existing_accounts
        .iter()
        .filter(|account| !account_ids.contains(&account.account.id))
        .cloned()
        .collect::<Vec<_>>();
    zenith_relay_core::accounts::removal::remove_accounts(
        &DesktopRemoval {
            state: &state,
            credentials: &credentials,
            account_ids: &account_ids,
            initial_wake,
            initial_automations,
            initial_proxy_pool,
            accounts,
            keys: old_keys,
            automations,
        },
        &account_ids,
    )
    .await?;
    let snapshot = state.snapshot().await?;
    drop(_mutation);
    refresh_active_codex_catalog_in_background(app);
    Ok(snapshot)
}

struct DesktopRemoval<'a> {
    state: &'a DesktopState,
    credentials: &'a CredentialStore<NativeSecretBackend>,
    account_ids: &'a [String],
    initial_wake: zenith_relay_core::automations::WakeCoordinator,
    initial_automations: AutomationRecords,
    initial_proxy_pool: ProxyPool,
    accounts: Vec<LocalAccountRecord>,
    keys: Vec<LocalGatewayKeyRecord>,
    automations: AutomationRecords,
}

impl zenith_relay_core::accounts::removal::RemovalHost for DesktopRemoval<'_> {
    type Prepared = prepare::PreparedAccountDelete;
    type Error = LocalPoolError;

    async fn prepare(&self, id: &str) -> LocalResult<Self::Prepared> {
        prepare_delete_local_account(id, self.state, self.credentials).await
    }
    fn commit(&self) -> LocalResult<()> {
        self.state.store()?.delete_accounts_state(
            self.account_ids,
            self.accounts.clone(),
            self.keys.clone(),
            self.automations.clone(),
        )
    }
    async fn rollback(
        &self,
        prepared: &[Self::Prepared],
        cause: &LocalPoolError,
    ) -> LocalResult<()> {
        ensure_delete_rollback_or_fail_closed(
            self.state,
            rollback_batch_delete(
                self.state,
                self.credentials,
                prepared,
                self.initial_wake.clone(),
                self.initial_automations.clone(),
                &self.initial_proxy_pool,
                cause,
            ),
        )
        .await
    }
    async fn retire(&self) {
        if let Some(runtime) = self.state.gateway.runtime().await {
            for id in self.account_ids {
                runtime.remove_candidate(id);
            }
        }
        for id in self.account_ids {
            self.state.token_authority().remove(id);
            let _ = self.state.remove_quota_account_lock(id);
        }
    }
}

#[cfg(test)]
mod tests;
