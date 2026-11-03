#[cfg(test)]
use super::{
    accounts::{NativeSecretBackend, credentials::CredentialStore},
    commands::pool,
    models::LocalGatewayKeyRecord,
    store::secret_store,
};
use super::{
    error::{LocalPoolError, Result},
    models::{LocalAccountRecord, ProviderSourceRecord},
    profiles,
    state::DesktopState,
    store::gateway_keys,
};
use std::collections::BTreeSet;
use zenith_relay_core::{
    ExecutionFence, GatewayRuntime, QUOTA_STALE_AFTER_MS, RuntimeCandidatePolicy,
    accounts::AccountRecord,
    changed_runtime_source_policy_updates,
    protocol::{
        AccountOperationalInput, AccountOperationalState, account_candidate_enabled,
        account_operational_state,
    },
};
#[cfg(test)]
use zenith_relay_core::{WireApi, protocol::AccountRoutingBlockReason};

pub(in crate::local_pool) use zenith_relay_core::unix_time_ms as current_time_ms;

mod assemble;
mod quota_references;
mod sync;

#[cfg(test)]
use assemble::{managed_chatgpt_account_id_for_reserve, timestamp_ms};

pub(in crate::local_pool) use assemble::runtime_from_store;
pub(in crate::local_pool) use quota_references::sync_account_quota_references;
pub(in crate::local_pool) use sync::{
    core_error, fail_closed, queue_active_client_catalog_refresh,
    refresh_active_codex_catalog_in_background, restart_after_secret_change, restart_or_rollback,
    sync_account_or_rollback, sync_gateway_or_rollback, sync_records_or_rollback,
    sync_refreshed_account_or_rollback, sync_running_account_states, sync_runtime_account_state,
};

pub(in crate::local_pool) fn record_catalog_refresh_result(
    state: &DesktopState,
    refresh_status_result: &std::result::Result<
        profiles::CodexCatalogRefreshStatus,
        LocalPoolError,
    >,
) {
    match refresh_status_result {
        Ok(profiles::CodexCatalogRefreshStatus::Deferred) => {
            state.record_catalog_refresh_deferred()
        }
        Ok(_) => state.record_catalog_refresh_result(None),
        Err(error) => state.record_catalog_refresh_result(Some(error)),
    }
}

pub(in crate::local_pool) fn runtime_account_operational_state(
    account: &AccountRecord,
    now_ms: u64,
) -> AccountOperationalState {
    account_operational_state(AccountOperationalInput::from_source(
        account,
        true,
        true,
        now_ms,
        QUOTA_STALE_AFTER_MS,
    ))
}

pub(in crate::local_pool) async fn apply_source_policy_if_running(
    state: &DesktopState,
    previous_sources: &[ProviderSourceRecord],
    source: &ProviderSourceRecord,
) -> bool {
    apply_source_policies_if_running(state, previous_sources, std::slice::from_ref(source)).await
}

pub(in crate::local_pool) async fn apply_source_policies_if_running(
    state: &DesktopState,
    previous_sources: &[ProviderSourceRecord],
    sources: &[ProviderSourceRecord],
) -> bool {
    let Some(runtime) = state.gateway.runtime().await else {
        return true;
    };
    let updates = changed_runtime_source_policy_updates(previous_sources, sources);
    updates.is_empty() || runtime.update_source_policies(&updates)
}

/// Applies the configured Responses pool to an existing runtime. The live
/// scheduler is the source of truth for candidate availability, so this never
/// reopens credentials merely to rebuild a key scope.
pub(in crate::local_pool) fn apply_local_gateway_key_scope(
    state: &DesktopState,
    runtime: &GatewayRuntime,
) -> Result<bool> {
    let system_key = gateway_keys::ensure_system_gateway_key(state)?;
    let (sources, accounts, settings) = {
        let store = state.store()?;
        (
            store.sources().to_vec(),
            store.accounts().to_vec(),
            store.gateway().clone(),
        )
    };
    let (source_ids, account_ids) = local_pool_member_ids(&sources, &accounts)?;
    // Authorization follows configured membership. Temporary auth failures,
    // cooldowns and disables are enforced by the scheduler and must recover
    // without a second membership edit.
    let scope = zenith_relay_core::CandidateScope {
        source_ids: Some(source_ids),
        account_ids: Some(account_ids),
        model_rules: Default::default(),
    };
    runtime
        .set_pool_routing_policy_with_key_scopes(
            settings.pool_routing_for(&sources, &accounts),
            settings.max_retry_candidates,
            &[(system_key.id, scope)],
        )
        .map_err(core_error)
}

/// Keep pending final dispatches on the old physical routes closed while a
/// desktop command persists and publishes changed permissions or transport.
/// The returned guards must outlive the save, hot apply and any rollback.
pub(in crate::local_pool) fn fence_runtime_candidates(
    runtime: Option<&GatewayRuntime>,
    account_ids: &[String],
    source_ids: &[String],
) -> Vec<ExecutionFence> {
    let Some(runtime) = runtime else {
        return Vec::new();
    };
    let mut fences = account_ids
        .iter()
        .filter_map(|account_id| runtime.fence_candidate_dispatch(account_id))
        .collect::<Vec<_>>();
    for source_id in source_ids {
        fences.extend(runtime.fence_source_dispatch(source_id));
    }
    fences
}

/// Refreshes the managed local key's candidate scope without replacing the
/// listener or any source/account executor. Source membership is represented
/// by this scope, so a policy-only source edit that also changes `in_pool`
/// does not need a gateway restart.
pub(in crate::local_pool) async fn refresh_local_gateway_key_scope_if_running(
    state: &DesktopState,
) -> Result<bool> {
    let Some(runtime) = state.gateway.runtime().await else {
        return Ok(true);
    };
    apply_local_gateway_key_scope(state, &runtime)
}

pub(in crate::local_pool) async fn apply_account_policy_if_running(
    state: &DesktopState,
    account: &LocalAccountRecord,
) -> bool {
    let Some(runtime) = state.gateway.runtime().await else {
        return true;
    };
    runtime.update_account_policy(
        &account.account.id,
        runtime_account_policy(account, current_time_ms()),
    )
}

/// Refresh authentication health and quota from the current durable record.
/// Policy edits use a separate path so changing a label or priority cannot
/// clear a scheduler failure observed during an in-flight request.
pub(in crate::local_pool) async fn sync_account_state_if_running(
    state: &DesktopState,
    account_id: &str,
) -> bool {
    let Some(runtime) = state.gateway.runtime().await else {
        return true;
    };
    let Ok(store) = state.store() else {
        return false;
    };
    if quota_references::sync_account_quota_references(state, &runtime, &store).is_err() {
        return false;
    }
    store.account(account_id).is_some_and(|account| {
        !store.gateway().account_route_is_selected(account_id)
            || sync_runtime_account_state(&runtime, account, current_time_ms())
    })
}

/// Maps the persisted account state into the part of a live candidate that can
/// change without replacing its OAuth executor. Pool membership affects this
/// policy through `runtime_account_operational_state`, so adding or removing
/// an account from the local pool can be applied without restarting the
/// listener or interrupting active streams.
pub(in crate::local_pool) fn runtime_account_policy(
    account: &LocalAccountRecord,
    now_ms: u64,
) -> RuntimeCandidatePolicy {
    let operational = runtime_account_operational_state(&account.account, now_ms);
    RuntimeCandidatePolicy {
        enabled: account_candidate_enabled(
            account.account.enabled && account.recovery.is_none(),
            operational.routing_block_reason,
        ),
        draining: account.account.draining,
        priority: account.priority,
        weight: account.weight,
        allowed_models: account.allowed_models.clone(),
        excluded_models: account.excluded_models.clone(),
    }
}

/// Return configured pool members without consulting live scheduler health.
/// Availability is intentionally a runtime concern and is applied later.
pub(in crate::local_pool) fn local_pool_member_ids(
    sources: &[ProviderSourceRecord],
    accounts: &[LocalAccountRecord],
) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let mut source_ids = BTreeSet::new();
    for source in sources.iter().filter(|source| source.in_pool) {
        // A damaged legacy binding is an unavailable candidate, not a reason
        // to reject the complete local key scope. Runtime admission records a
        // stable source error and keeps the remaining pool routes usable.
        if source.supports_any_wire_api().unwrap_or(false) {
            source_ids.insert(source.id.clone());
        }
    }
    let account_ids = accounts
        .iter()
        .filter(|account| account.account.in_pool && account.recovery.is_none())
        .map(|account| account.account.id.clone())
        .collect();
    Ok((source_ids, account_ids))
}

#[cfg(test)]
mod tests;
