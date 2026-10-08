use crate::app::account_proxy_config;
use crate::state::{now_ms, AccountCredential, AppState, ServerAccountRecord};
use std::collections::BTreeSet;
use zenith_relay_core::protocol::{
    account_candidate_enabled, account_operational_state, AccountOperationalInput,
};
use zenith_relay_core::{
    pool_dispatch_permission_changed, CandidateKind, PoolParticipant, RuntimeCandidatePolicy,
};

pub(super) fn apply_account_policy_if_running(
    state: &AppState,
    account: &ServerAccountRecord,
) -> Result<bool, String> {
    apply_account_policies_if_running(state, std::slice::from_ref(account))
}

/// Applies account policies before widening or narrowing the internal key
/// scope. Account membership is part of an account candidate's operational
/// state, unlike API-source membership which is enforced solely by the key
/// scope. Updating the candidate first means a removed account cannot accept a
/// new request during the scope update, while an in-flight request keeps its
/// existing executor.
pub(super) fn apply_account_policies_if_running(
    state: &AppState,
    accounts: &[ServerAccountRecord],
) -> Result<bool, String> {
    let Some(runtime) = state.runtime()? else {
        return Ok(!state.store.gateway_enabled()?);
    };
    let candidate_ids = runtime
        .candidate_runtime_order()
        .into_iter()
        .filter(|candidate| candidate.kind == CandidateKind::OAuthAccount)
        .map(|candidate| candidate.candidate_id)
        .collect::<BTreeSet<_>>();
    for account in accounts {
        let policy = account_runtime_policy(state, account)?;
        if !candidate_ids.contains(&account.id) {
            if policy.enabled {
                return Ok(false);
            }
            continue;
        }
        if !runtime.update_account_policy(&account.id, policy) {
            return Ok(false);
        }
    }
    state.refresh_internal_gateway_key_scopes(&runtime)
}

fn account_runtime_policy(
    state: &AppState,
    account: &ServerAccountRecord,
) -> Result<RuntimeCandidatePolicy, String> {
    let credential = state
        .vault
        .load(&account.secret_ref)?
        .and_then(|credential_json| {
            serde_json::from_str::<AccountCredential>(&credential_json).ok()
        });
    let secret_available = credential.is_some();
    let proxy_available = credential
        .as_ref()
        .is_some_and(|credential| account_proxy_config(state, account, credential).is_ok());
    let operational = account_operational_state(AccountOperationalInput::from_source(
        account,
        secret_available,
        proxy_available,
        now_ms(),
        zenith_relay_core::QUOTA_STALE_AFTER_MS,
    ));
    Ok(RuntimeCandidatePolicy {
        enabled: account_candidate_enabled(account.enabled, operational.routing_block_reason),
        draining: account.draining,
        priority: account.priority,
        weight: account.weight,
        allowed_models: account.allowed_models.clone(),
        excluded_models: account.excluded_models.clone(),
    })
}

pub(super) fn account_runtime_policy_changed(
    previous_account: &ServerAccountRecord,
    updated_account: &ServerAccountRecord,
) -> bool {
    previous_account.enabled != updated_account.enabled
        || previous_account.draining != updated_account.draining
        || previous_account.priority != updated_account.priority
        || previous_account.weight != updated_account.weight
        || previous_account.allowed_models != updated_account.allowed_models
        || previous_account.excluded_models != updated_account.excluded_models
}

pub(super) fn account_dispatch_permission_changed(
    previous_account: &ServerAccountRecord,
    updated_account: &ServerAccountRecord,
) -> bool {
    pool_dispatch_permission_changed(
        previous_account.pool_access(),
        updated_account.pool_access(),
    )
}
