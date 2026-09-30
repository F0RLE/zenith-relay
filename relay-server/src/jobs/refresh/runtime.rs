use super::{AccountRefreshFence, AppState};
use crate::{
    app::account_proxy_config,
    state::{now_ms, AccountCredential},
};
use std::sync::Arc;
use zenith_relay_core::{
    protocol::{account_candidate_enabled, account_operational_state, AccountOperationalInput},
    QUOTA_STALE_AFTER_MS,
};

/// Ordinary reads must not replace the scheduler that owns active leases,
/// recovery state, request budgets and stream affinity.
pub(super) async fn synchronize(
    state: &Arc<AppState>,
    fence: &AccountRefreshFence,
    models_changed: bool,
    health_changed: bool,
) -> Result<(), String> {
    let _configuration = state.configuration_lock.lock().await;
    let (account, current) = state.store.account_refresh_scope(&fence.account_id)?;
    if &current != fence {
        return Err("account changed during refresh".into());
    }
    let Some(runtime) = state.runtime()? else {
        return state.rebuild_runtime().await;
    };
    if models_changed && !runtime.update_account_models(&account.id, account.effective_models()) {
        return state.rebuild_runtime().await;
    }
    let credential = state
        .vault
        .load(&account.secret_ref)?
        .and_then(|value| serde_json::from_str::<AccountCredential>(&value).ok());
    let secret_available = credential.is_some();
    let proxy_available = credential
        .as_ref()
        .is_some_and(|credential| account_proxy_config(state, &account, credential).is_ok());
    let now = now_ms();
    let operational = account_operational_state(AccountOperationalInput {
        enabled: account.enabled,
        in_pool: account.in_pool,
        draining: account.draining,
        secret_available,
        proxy_available,
        auth_state: account.auth_state,
        health: account.health,
        subscription: &account.subscription,
        quota: &account.quota,
        last_error_code: account.last_error_code.as_deref(),
        now_ms: now,
        quota_stale_after_ms: QUOTA_STALE_AFTER_MS,
    });
    let enabled = account_candidate_enabled(account.enabled, operational.routing_block_reason);
    let synced = if !health_changed {
        runtime.sync_account_refresh_availability_with_quota(
            &account.id,
            enabled,
            operational.health,
            &account.quota,
            now,
        )
    } else {
        runtime.sync_account_availability_with_quota(
            &account.id,
            enabled,
            operational.health,
            &account.quota,
            now,
        )
    };
    if !synced && enabled {
        // The member may have recovered from a missing credential at startup.
        return state.rebuild_runtime().await;
    }
    Ok(())
}
