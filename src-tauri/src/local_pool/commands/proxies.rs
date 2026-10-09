use super::{current_time_ms, fence_runtime_candidates, restart_or_rollback};
use crate::local_pool::{
    accounts::{
        credentials::CredentialStore,
        proxy::{ProxyPool, ProxyPoolSummary},
        NativeSecretBackend,
    },
    error::CommandError,
    state::DesktopState,
};
use std::collections::HashSet;
use tauri::State;

mod apply;
mod types;

pub(crate) use apply::set_account_proxy_inner;
use apply::{apply_choices, load_reconciled_pool, normalize_ids};
use types::{
    AssignFreeProxiesInput, AssignStoredProxyInput, DeleteStoredProxiesInput, ImportProxyPoolInput,
    ProxyChoice, ProxyPoolImportResult, SetStoredProxyAccountsInput, StoredProxyAssignmentResult,
};

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
    let previous_ids: HashSet<_> = pool
        .summary()
        .entries
        .into_iter()
        .map(|proxy_summary| proxy_summary.id)
        .collect();
    let (added, duplicates) = pool.import(&input.proxy_urls, current_time_ms())?;
    pool.save()?;
    let summary = pool.summary();
    Ok(ProxyPoolImportResult {
        added,
        duplicates,
        added_proxy_ids: summary
            .entries
            .iter()
            .filter(|proxy_summary| !previous_ids.contains(&proxy_summary.id))
            .map(|proxy_summary| proxy_summary.id.clone())
            .collect(),
        pool: summary,
    })
}

#[tauri::command]
pub async fn check_local_stored_proxy(
    proxy_id: String,
    state: State<'_, DesktopState>,
) -> std::result::Result<crate::local_pool::accounts::proxy::check::ProxyCheckResult, CommandError>
{
    let proxy_id = proxy_id.trim().to_owned();
    let (proxy, expected_url) = {
        let _mutation = state.setup_guard().await;
        let pool = ProxyPool::load()?;
        (pool.config(&proxy_id)?, pool.stored_url(&proxy_id)?)
    };
    let result =
        crate::local_pool::accounts::proxy::check::check(proxy_id, &proxy, current_time_ms()).await;
    let _mutation = state.setup_guard().await;
    let mut pool = ProxyPool::load()?;
    if pool.record_check(&expected_url, result.clone()) {
        pool.save()?;
    }
    Ok(result)
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
pub async fn delete_local_stored_proxies(
    input: DeleteStoredProxiesInput,
    state: State<'_, DesktopState>,
) -> std::result::Result<ProxyPoolSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let proxy_ids = normalize_ids(input.proxy_ids, false)?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let mut pool = load_reconciled_pool(&state, &credentials)?;
    pool.delete_many(&proxy_ids)?;
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
pub async fn set_local_stored_proxy_accounts(
    input: SetStoredProxyAccountsInput,
    state: State<'_, DesktopState>,
) -> std::result::Result<StoredProxyAssignmentResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let proxy_id = input.proxy_id.trim().to_string();
    let account_ids = normalize_ids(input.account_ids, true)?;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let currently_assigned_account_ids =
        load_reconciled_pool(&state, &credentials)?.assigned_account_ids(&proxy_id)?;
    let selected = account_ids.iter().cloned().collect::<HashSet<_>>();
    let mut choices = currently_assigned_account_ids
        .into_iter()
        .filter(|account_id| !selected.contains(account_id.as_str()))
        .map(|account_id| (account_id, ProxyChoice::Inherited))
        .collect::<Vec<_>>();
    choices.extend(
        account_ids
            .into_iter()
            .map(|account_id| (account_id, ProxyChoice::Stored(proxy_id.clone()))),
    );
    apply_choices(&state, choices).await.map_err(Into::into)
}

#[tauri::command]
pub async fn assign_free_local_account_proxies(
    input: AssignFreeProxiesInput,
    state: State<'_, DesktopState>,
) -> std::result::Result<StoredProxyAssignmentResult, CommandError> {
    let _mutation = state.setup_guard().await;
    let account_ids = normalize_ids(input.account_ids, false)?;
    apply_choices(
        &state,
        account_ids
            .into_iter()
            .map(|account_id| (account_id, ProxyChoice::Automatic))
            .collect(),
    )
    .await
    .map_err(Into::into)
}
