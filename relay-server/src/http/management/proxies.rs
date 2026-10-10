use super::{account_summary, find_account, runtime_error, store_error, ManagementError};
use crate::state::{ensure_proxy_record, AppState, MAX_SERVER_ACCOUNTS};
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use zenith_relay_core::error_codes;
use zenith_relay_core::protocol::{AccountSummary, RuntimeStateSnapshot};
use zenith_relay_core::{normalize_proxy_url, ExecutionFence};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/proxies/common", post(set_common_proxy))
        .route("/proxies/policy", post(set_account_proxy_required))
        .route("/accounts/proxies/assign", post(assign_account_proxies))
        .route("/accounts/{id}/proxy", post(set_account_proxy))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SetProxyInput {
    proxy_url: Option<String>,
    #[serde(default)]
    bypass_common_proxy: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyPolicyInput {
    required: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AssignProxiesInput {
    account_ids: Vec<String>,
    proxy_urls: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyAssignmentResult {
    assigned: usize,
    unused: usize,
}

pub async fn set_common_proxy(
    State(state): State<Arc<AppState>>,
    Json(input): Json<SetProxyInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let requested_proxy_url = normalize_optional_proxy(input.proxy_url)?;
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let previous_proxy_id = state.store.common_proxy_id().map_err(store_error)?;
    let next_proxy_id = requested_proxy_url
        .as_deref()
        .map(|proxy_url| ensure_proxy_record(&state.store, &state.vault, proxy_url))
        .transpose()
        .map_err(store_error)?
        .map(|proxy_record| proxy_record.id);
    if previous_proxy_id == next_proxy_id {
        build.rebuild(&state).await.map_err(runtime_error)?;
        return Ok(Json(state.snapshot().map_err(store_error)?));
    }
    let accounts = state.store.accounts().map_err(store_error)?;
    let _dispatch_fences = fence_accounts(
        &state,
        accounts
            .iter()
            .filter(|account| account.proxy_id.is_none() && !account.bypass_common_proxy)
            .map(|account| account.id.as_str()),
    )?;
    state
        .store
        .set_common_proxy_id(next_proxy_id.as_deref())
        .map_err(store_error)?;
    build
        .rebuild_or_rollback(&state, || {
            state
                .store
                .set_common_proxy_id(previous_proxy_id.as_deref())
        })
        .await
        .map_err(runtime_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn set_account_proxy_required(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ProxyPolicyInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let previous_proxy_required = state.store.account_proxy_required().map_err(store_error)?;
    if previous_proxy_required == input.required {
        return Ok(Json(state.snapshot().map_err(store_error)?));
    }
    let accounts = state.store.accounts().map_err(store_error)?;
    let _dispatch_fences = fence_accounts(
        &state,
        accounts
            .iter()
            .filter(|account| account.proxy_id.is_none())
            .map(|account| account.id.as_str()),
    )?;
    state
        .store
        .set_account_proxy_required(input.required)
        .map_err(store_error)?;
    build
        .rebuild_or_rollback(&state, || {
            state
                .store
                .set_account_proxy_required(previous_proxy_required)
        })
        .await
        .map_err(runtime_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn set_account_proxy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SetProxyInput>,
) -> Result<Json<AccountSummary>, ManagementError> {
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let mut account_record = find_account(&state, &id)?;
    if input.proxy_url.is_some() && input.bypass_common_proxy {
        return Err(ManagementError::validation(
            error_codes::PROXY_ROUTE_AMBIGUOUS,
            "an account route cannot use and bypass a proxy at the same time",
        ));
    }
    let previous_proxy_state = (
        account_record.proxy_id.clone(),
        account_record.bypass_common_proxy,
    );
    let requested_proxy_url = normalize_optional_proxy(input.proxy_url)?;
    let next_proxy_id = requested_proxy_url
        .as_deref()
        .map(|proxy_url| ensure_proxy_record(&state.store, &state.vault, proxy_url))
        .transpose()
        .map_err(store_error)?
        .map(|proxy| proxy.id);
    if previous_proxy_state == (next_proxy_id.clone(), input.bypass_common_proxy) {
        return Ok(Json(account_summary(&state, &account_record)?));
    }
    account_record.proxy_id = next_proxy_id;
    account_record.bypass_common_proxy = input.bypass_common_proxy;
    let _dispatch_fences = fence_accounts(&state, std::iter::once(account_record.id.as_str()))?;
    state
        .store
        .save_account(&account_record)
        .map_err(store_error)?;
    let mut restored_account = account_record.clone();
    build
        .rebuild_or_rollback(&state, || {
            restored_account.proxy_id = previous_proxy_state.0.clone();
            restored_account.bypass_common_proxy = previous_proxy_state.1;
            state.store.save_account(&restored_account)
        })
        .await
        .map_err(runtime_error)?;
    Ok(Json(account_summary(&state, &account_record)?))
}

pub async fn assign_account_proxies(
    State(state): State<Arc<AppState>>,
    Json(input): Json<AssignProxiesInput>,
) -> Result<Json<ProxyAssignmentResult>, ManagementError> {
    if input.account_ids.is_empty()
        || input.account_ids.len() > MAX_SERVER_ACCOUNTS
        || input.proxy_urls.len() < input.account_ids.len()
    {
        return Err(ManagementError::validation(
            error_codes::PROXY_ASSIGNMENT_INVALID,
            "proxy list must contain one URL per selected account",
        ));
    }
    let mut seen = HashSet::new();
    if input
        .account_ids
        .iter()
        .any(|account_id| !seen.insert(account_id.clone()))
    {
        return Err(ManagementError::validation(
            error_codes::PROXY_ASSIGNMENT_DUPLICATE,
            "proxy assignment contains duplicate account ids",
        ));
    }
    let _configuration = state.configuration_lock.lock().await;
    let build = state.lock_runtime_rebuild().await;
    let previous_records = input
        .account_ids
        .iter()
        .map(|account_id| find_account(&state, account_id))
        .collect::<Result<Vec<_>, _>>()?;
    let mut updated_accounts = Vec::with_capacity(input.account_ids.len());
    for (account_id, proxy_url) in input.account_ids.iter().zip(&input.proxy_urls) {
        let mut account_record = previous_records
            .iter()
            .find(|account_record| &account_record.id == account_id)
            .cloned()
            .ok_or_else(|| {
                ManagementError::not_found(error_codes::ACCOUNT_NOT_FOUND, "account not found")
            })?;
        let proxy = ensure_proxy_record(&state.store, &state.vault, &normalize_proxy(proxy_url)?)
            .map_err(store_error)?;
        account_record.proxy_id = Some(proxy.id);
        updated_accounts.push(account_record);
    }
    let _dispatch_fences = fence_accounts(
        &state,
        previous_records.iter().zip(&updated_accounts).filter_map(
            |(previous_record, updated_record)| {
                (previous_record.proxy_id != updated_record.proxy_id)
                    .then_some(updated_record.id.as_str())
            },
        ),
    )?;
    state
        .store
        .save_accounts(&updated_accounts)
        .map_err(store_error)?;
    build
        .rebuild_or_rollback(&state, || state.store.save_accounts(&previous_records))
        .await
        .map_err(runtime_error)?;
    Ok(Json(ProxyAssignmentResult {
        assigned: updated_accounts.len(),
        unused: input
            .proxy_urls
            .len()
            .saturating_sub(updated_accounts.len()),
    }))
}

fn normalize_optional_proxy(proxy_url: Option<String>) -> Result<Option<String>, ManagementError> {
    proxy_url
        .map(|proxy_url| normalize_proxy(&proxy_url))
        .transpose()
}

fn fence_accounts<'a>(
    state: &AppState,
    account_ids: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<ExecutionFence>, ManagementError> {
    let Some(runtime) = state.runtime().map_err(runtime_error)? else {
        return Ok(Vec::new());
    };
    Ok(account_ids
        .into_iter()
        .filter_map(|account_id| runtime.fence_candidate_dispatch(account_id))
        .collect())
}

fn normalize_proxy(proxy_url: &str) -> Result<String, ManagementError> {
    normalize_proxy_url(proxy_url)
        .map_err(|message| ManagementError::validation(error_codes::PROXY_INVALID, message))
}
