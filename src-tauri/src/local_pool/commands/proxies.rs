use super::{current_time_ms, fence_runtime_candidates, restart_or_rollback};
use crate::local_pool::{
    accounts::{
        credentials::CredentialStore,
        proxy::{account_proxy_pool_id, common_proxy_pool_id, ProxyPool, ProxyPoolSummary},
        NativeSecretBackend,
    },
    error::CommandError,
    state::DesktopState,
};
use std::collections::HashSet;
use tauri::State;

mod apply;
mod timezone;
mod types;

pub(crate) use apply::set_account_proxy_inner;
use apply::{apply_choices, load_reconciled_pool, normalize_ids};
use types::{
    AssignFreeProxiesInput, AssignStoredProxyInput, DeleteStoredProxiesInput, ImportProxyPoolInput,
    ProxyChoice, ProxyPoolImportResult, SetStoredProxyAccountsInput, SetStoredProxyTimezoneInput,
    StoredProxyAssignmentResult,
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
    let mut result =
        crate::local_pool::accounts::proxy::check::check(proxy_id, &proxy, current_time_ms()).await;
    let _mutation = state.setup_guard().await;
    let mut pool = ProxyPool::load()?;
    let previous_timezone = pool.timezone(&result.proxy_id).ok().flatten();
    if pool.record_check(&expected_url, result.clone()) {
        if pool.timezone(&result.proxy_id)? != previous_timezone {
            timezone::save_pool(&state, &pool, &result.proxy_id).await?;
        } else {
            pool.save()?;
        }
        result.timezone = pool.timezone(&result.proxy_id)?;
    }
    Ok(result)
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
pub async fn set_local_stored_proxy_timezone(
    input: SetStoredProxyTimezoneInput,
    state: State<'_, DesktopState>,
) -> std::result::Result<ProxyPoolSummary, CommandError> {
    let _mutation = state.setup_guard().await;
    let mut pool = ProxyPool::load()?;
    let changed = pool.set_timezone(input.proxy_id.trim(), input.timezone)?;
    if !changed {
        return Ok(pool.summary());
    }
    timezone::save_pool(&state, &pool, input.proxy_id.trim()).await?;
    Ok(pool.summary())
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
