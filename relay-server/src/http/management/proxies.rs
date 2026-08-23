use super::{account_summary, find_account, runtime_error, store_error, ManagementError};
use crate::state::{ensure_proxy_record, AppState, MAX_SERVER_ACCOUNTS};
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use zenith_relay_core::normalize_proxy_url;
use zenith_relay_core::protocol::{AccountSummary, RuntimeStateSnapshot};

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
    let next = normalize_optional_proxy(input.proxy_url)?;
    let previous = state.store.common_proxy_id().map_err(store_error)?;
    let next = next
        .as_deref()
        .map(|value| ensure_proxy_record(&state.store, &state.vault, value))
        .transpose()
        .map_err(store_error)?
        .map(|record| record.id);
    if previous == next {
        state.rebuild_runtime().await.map_err(runtime_error)?;
        return Ok(Json(state.snapshot().map_err(store_error)?));
    }
    state
        .store
        .set_common_proxy_id(next.as_deref())
        .map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| state.store.set_common_proxy_id(previous.as_deref()))
        .await
        .map_err(runtime_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn set_account_proxy_required(
    State(state): State<Arc<AppState>>,
    Json(input): Json<ProxyPolicyInput>,
) -> Result<Json<RuntimeStateSnapshot>, ManagementError> {
    let previous = state.store.account_proxy_required().map_err(store_error)?;
    if previous == input.required {
        return Ok(Json(state.snapshot().map_err(store_error)?));
    }
    state
        .store
        .set_account_proxy_required(input.required)
        .map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| state.store.set_account_proxy_required(previous))
        .await
        .map_err(runtime_error)?;
    Ok(Json(state.snapshot().map_err(store_error)?))
}

pub async fn set_account_proxy(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<SetProxyInput>,
) -> Result<Json<AccountSummary>, ManagementError> {
    let mut record = find_account(&state, &id)?;
    if input.proxy_url.is_some() && input.bypass_common_proxy {
        return Err(ManagementError::validation(
            "proxy_route_ambiguous",
            "an account route cannot use and bypass a proxy at the same time",
        ));
    }
    let previous = (record.proxy_id.clone(), record.bypass_common_proxy);
    let next = normalize_optional_proxy(input.proxy_url)?;
    let next = next
        .as_deref()
        .map(|value| ensure_proxy_record(&state.store, &state.vault, value))
        .transpose()
        .map_err(store_error)?
        .map(|proxy| proxy.id);
    if previous == (next.clone(), input.bypass_common_proxy) {
        return Ok(Json(account_summary(&state, &record)?));
    }
    record.proxy_id = next;
    record.bypass_common_proxy = input.bypass_common_proxy;
    state.store.save_account(&record).map_err(store_error)?;
    let mut restored = record.clone();
    state
        .rebuild_runtime_or_rollback(|| {
            restored.proxy_id = previous.0.clone();
            restored.bypass_common_proxy = previous.1;
            state.store.save_account(&restored)
        })
        .await
        .map_err(runtime_error)?;
    Ok(Json(account_summary(&state, &record)?))
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
            "proxy_assignment_invalid",
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
            "proxy_assignment_duplicate",
            "proxy assignment contains duplicate account ids",
        ));
    }
    let old_records = input
        .account_ids
        .iter()
        .map(|account_id| find_account(&state, account_id))
        .collect::<Result<Vec<_>, _>>()?;
    let mut updates = Vec::with_capacity(input.account_ids.len());
    for (account_id, proxy_url) in input.account_ids.iter().zip(&input.proxy_urls) {
        let mut record = old_records
            .iter()
            .find(|record| &record.id == account_id)
            .cloned()
            .ok_or_else(|| ManagementError::not_found("account_not_found", "account not found"))?;
        let proxy = ensure_proxy_record(&state.store, &state.vault, &normalize_proxy(proxy_url)?)
            .map_err(store_error)?;
        record.proxy_id = Some(proxy.id);
        updates.push(record);
    }
    state.store.save_accounts(&updates).map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| state.store.save_accounts(&old_records))
        .await
        .map_err(runtime_error)?;
    Ok(Json(ProxyAssignmentResult {
        assigned: updates.len(),
        unused: input.proxy_urls.len().saturating_sub(updates.len()),
    }))
}

fn normalize_optional_proxy(value: Option<String>) -> Result<Option<String>, ManagementError> {
    value.map(|value| normalize_proxy(&value)).transpose()
}

fn normalize_proxy(value: &str) -> Result<String, ManagementError> {
    normalize_proxy_url(value)
        .map_err(|message| ManagementError::validation("proxy_invalid", message))
}
