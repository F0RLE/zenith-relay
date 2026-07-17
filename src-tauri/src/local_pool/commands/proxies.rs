use super::{current_time_ms, restart_or_rollback};
use crate::local_pool::{
    accounts::{
        credentials::{CredentialError, CredentialStore, StoredCodexCredentials},
        proxy::{ProxyPool, ProxyPoolSummary},
        NativeSecretBackend,
    },
    error::{CommandError, ErrorCode, LocalPoolError, Result},
    state::DesktopState,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use tauri::State;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportProxyPoolInput {
    proxy_urls: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignStoredProxyInput {
    account_id: String,
    proxy_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignFreeProxiesInput {
    account_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyPoolImportResult {
    pub added: usize,
    pub duplicates: usize,
    pub pool: ProxyPoolSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredProxyAssignmentResult {
    pub assigned: usize,
    pub unchanged: usize,
    pub unavailable: usize,
    pub pool: ProxyPoolSummary,
}

enum ProxyChoice {
    Inherited,
    Free,
    Stored(String),
    Custom(String),
}

#[tauri::command]
pub async fn get_local_proxy_pool(
    state: State<'_, DesktopState>,
) -> std::result::Result<ProxyPoolSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    Ok(load_reconciled_pool(&state, &credentials)?.summary())
}

#[tauri::command]
pub async fn import_local_proxy_pool(
    input: ImportProxyPoolInput,
    state: State<'_, DesktopState>,
) -> std::result::Result<ProxyPoolImportResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let mut pool = load_reconciled_pool(&state, &credentials)?;
    let (added, duplicates) = pool.import(&input.proxy_urls, current_time_ms())?;
    pool.save()?;
    Ok(ProxyPoolImportResult {
        added,
        duplicates,
        pool: pool.summary(),
    })
}

#[tauri::command]
pub async fn delete_local_stored_proxy(
    proxy_id: String,
    state: State<'_, DesktopState>,
) -> std::result::Result<ProxyPoolSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let mut pool = load_reconciled_pool(&state, &credentials)?;
    pool.delete(proxy_id.trim())?;
    pool.save()?;
    Ok(pool.summary())
}

#[tauri::command]
pub async fn assign_local_stored_proxy(
    input: AssignStoredProxyInput,
    state: State<'_, DesktopState>,
) -> std::result::Result<StoredProxyAssignmentResult, CommandError> {
    let _mutation = state.setup_guard().await;
    apply_choices(
        &state,
        vec![(input.account_id, ProxyChoice::Stored(input.proxy_id))],
    )
    .await
    .map_err(Into::into)
}

#[tauri::command]
pub async fn assign_free_local_account_proxies(
    input: AssignFreeProxiesInput,
    state: State<'_, DesktopState>,
) -> std::result::Result<StoredProxyAssignmentResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let account_ids = normalize_account_ids(input.account_ids)?;
    apply_choices(
        &state,
        account_ids
            .into_iter()
            .map(|account_id| (account_id, ProxyChoice::Free))
            .collect(),
    )
    .await
    .map_err(Into::into)
}

pub(super) async fn set_account_proxy_inner(
    account_id: String,
    proxy_url: Option<String>,
    state: &DesktopState,
) -> Result<StoredProxyAssignmentResult> {
    let choice = proxy_url.map_or(ProxyChoice::Inherited, ProxyChoice::Custom);
    apply_choices(state, vec![(account_id, choice)]).await
}

async fn apply_choices(
    state: &DesktopState,
    choices: Vec<(String, ProxyChoice)>,
) -> Result<StoredProxyAssignmentResult> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let old_pool = load_reconciled_pool(state, &credentials)?;
    let mut pool = old_pool.clone();
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .map(|account| account.account.id.clone())
        .collect::<HashSet<_>>();
    let mut updates = Vec::new();
    let mut unchanged = 0;
    let mut unavailable = 0;
    for (account_id, choice) in choices {
        if !account_ids.contains(account_id.as_str()) {
            return Err(LocalPoolError::new(
                ErrorCode::NotFound,
                "account not found",
            ));
        }
        let old = credentials.require(&account_id).map_err(credential_error)?;
        let next_url = match choice {
            ProxyChoice::Inherited => {
                pool.release(&account_id);
                None
            }
            ProxyChoice::Free => match pool.assign_free(&account_id) {
                Some(url) => Some(url),
                None => {
                    unavailable += 1;
                    continue;
                }
            },
            ProxyChoice::Stored(proxy_id) => Some(pool.assign_id(&proxy_id, &account_id)?),
            ProxyChoice::Custom(value) => {
                Some(pool.assign_url(&value, &account_id, current_time_ms())?)
            }
        };
        let next = old
            .clone()
            .with_proxy_url(next_url)
            .map_err(credential_error)?;
        if old.proxy_url() == next.proxy_url() {
            unchanged += 1;
        } else {
            updates.push((old, next));
        }
    }
    if updates.is_empty() && pool == old_pool {
        return Ok(StoredProxyAssignmentResult {
            assigned: 0,
            unchanged,
            unavailable,
            pool: pool.summary(),
        });
    }
    for (_, next) in &updates {
        state.mark_quota_refresh(next.local_account_id(), current_time_ms())?;
    }
    save_credential_updates(&credentials, &updates)?;
    if let Err(error) = pool.save() {
        restore_credentials(&credentials, &updates)?;
        return Err(error);
    }
    let rollback_credentials = credentials.clone();
    let rollback_updates = updates.clone();
    restart_or_rollback(state, move || {
        restore_credentials(&rollback_credentials, &rollback_updates)?;
        old_pool.save()
    })
    .await?;
    Ok(StoredProxyAssignmentResult {
        assigned: updates.len(),
        unchanged,
        unavailable,
        pool: pool.summary(),
    })
}

fn load_reconciled_pool(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
) -> Result<ProxyPool> {
    let mut account_proxies = Vec::new();
    for account in state.store()?.accounts() {
        let proxy = credentials
            .load(&account.account.id)
            .map_err(credential_error)?
            .and_then(|stored| stored.proxy_url().map(str::to_string));
        account_proxies.push((account.account.id.clone(), proxy));
    }
    account_proxies.sort_by(|left, right| left.0.cmp(&right.0));
    let mut pool = ProxyPool::load()?;
    if pool.reconcile(&account_proxies, current_time_ms()) {
        pool.save()?;
    }
    Ok(pool)
}

fn save_credential_updates(
    credentials: &CredentialStore<NativeSecretBackend>,
    updates: &[(StoredCodexCredentials, StoredCodexCredentials)],
) -> Result<()> {
    for index in 0..updates.len() {
        if let Err(error) = credentials
            .save(&updates[index].1)
            .map_err(credential_error)
        {
            restore_credentials(credentials, &updates[..index])?;
            return Err(error);
        }
    }
    Ok(())
}

fn restore_credentials(
    credentials: &CredentialStore<NativeSecretBackend>,
    updates: &[(StoredCodexCredentials, StoredCodexCredentials)],
) -> Result<()> {
    for (old, _) in updates {
        credentials.save(old).map_err(credential_error)?;
    }
    Ok(())
}

fn normalize_account_ids(account_ids: Vec<String>) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let account_ids = account_ids
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if account_ids.is_empty() || account_ids.iter().any(|value| !seen.insert(value.clone())) {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account selection is empty or contains duplicates",
        ));
    }
    Ok(account_ids)
}

fn credential_error(error: CredentialError) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.to_string())
}
