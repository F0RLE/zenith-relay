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

use crate::local_pool::accounts::proxy::observations::load_reconciled_pool;
pub(crate) use apply::set_account_proxy_inner;
use apply::{apply_choices, normalize_ids};
use types::{
    AssignFreeProxiesInput, AssignStoredProxyInput, DeleteStoredProxiesInput, ImportProxyPoolInput,
    ProxyChoice, ProxyPoolImportResult, SetStoredProxyAccountsInput, StoredProxyAssignmentResult,
};

#[tauri::command]
pub async fn rename_local_stored_proxy(
    proxy_id: String,
    label: String,
    state: State<'_, DesktopState>,
) -> std::result::Result<ProxyPoolSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let mut pool = ProxyPool::load()?;
    pool.rename(proxy_id.trim(), &label)?;
    pool.save()?;
    Ok(pool.summary())
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
    let previous_ids: HashSet<_> = pool
        .summary()
        .entries
        .into_iter()
        .map(|proxy_summary| proxy_summary.id)
        .collect();
    let (added, duplicates) = pool.import(&input.proxy_urls, current_time_ms())?;
    pool.save()?;
    state.store()?.notify_refresh_changed();
    state.oauth_events.emit_proxy_pool_changed();
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
    crate::local_pool::refresh::proxies::request(&state, proxy_id.trim())
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn delete_local_stored_proxy(
    proxy_id: String,
    state: State<'_, DesktopState>,
) -> std::result::Result<ProxyPoolSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let proxy_ids = normalize_ids(vec![proxy_id], false)?;
    apply::delete_proxies(&state, &proxy_ids)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn test_local_stored_proxy_speed(
    proxy_id: String,
    on_started: tauri::ipc::Channel<()>,
    state: State<'_, DesktopState>,
) -> std::result::Result<
    crate::local_pool::accounts::proxy::speed::ProxySpeedTestResult,
    CommandError,
> {
    let proxy_id = proxy_id.trim().to_owned();
    let proxy = {
        let _mutation = state.setup_guard().await;
        ProxyPool::load()?.config(&proxy_id)?
    };
    Ok(
        crate::local_pool::accounts::proxy::speed::test(proxy_id, &proxy, || {
            on_started.send(()).is_ok()
        })
        .await,
    )
}

#[tauri::command]
pub async fn delete_local_stored_proxies(
    input: DeleteStoredProxiesInput,
    state: State<'_, DesktopState>,
) -> std::result::Result<ProxyPoolSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let proxy_ids = normalize_ids(input.proxy_ids, false)?;
    apply::delete_proxies(&state, &proxy_ids)
        .await
        .map_err(Into::into)
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
    apply::set_proxy_accounts(&state, &proxy_id, account_ids)
        .await
        .map_err(Into::into)
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
