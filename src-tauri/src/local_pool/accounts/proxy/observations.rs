use super::{account_proxy_pool_id, common_proxy_pool_id, common_proxy_url, ProxyPool};
use crate::local_pool::error::{LocalPoolError, Result};
use crate::local_pool::{
    accounts::{credentials::CredentialStore, NativeSecretBackend},
    state::DesktopState,
};

/// Reconcile legacy credential routes and the common route independently of UI.
pub(crate) fn load_reconciled_pool(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
) -> Result<ProxyPool> {
    let mut account_proxies = Vec::new();
    for account in state.store()?.accounts() {
        let proxy = credentials
            .load(&account.account.id)
            .map_err(crate::local_pool::accounts::credentials::credential_invalid_state_error)?
            .and_then(|stored| stored.proxy_url().map(str::to_string));
        account_proxies.push((account.account.id.clone(), proxy));
    }
    account_proxies.sort_by(|left, right| left.0.cmp(&right.0));
    let mut pool = ProxyPool::load()?;
    let now_ms = zenith_relay_core::unix_time_ms();
    let mut changed = pool.reconcile(&account_proxies, now_ms);
    if let Some(common) = common_proxy_url(state.store()?.gateway())? {
        changed |= pool.import(&[common], now_ms)?.0 > 0;
    }
    if changed {
        pool.save()?;
        state.store()?.notify_refresh_changed();
        state.oauth_events.emit_proxy_pool_changed();
    }
    Ok(pool)
}

/// Called only under the setup mutation guard. Resolve credential routes before
/// committing so a failed read cannot leave storage and the runtime out of sync.
pub(crate) async fn save_pool(
    state: &DesktopState,
    pool: &ProxyPool,
    proxy_id: &str,
) -> Result<()> {
    let timezone = pool.timezone_observation(proxy_id)?;
    let runtime = state.gateway.runtime().await;
    let mut affected_accounts = Vec::new();
    if runtime.is_some() {
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        let store = state.store()?;
        let common_proxy_id = common_proxy_pool_id(store.gateway(), pool);
        for account in store.accounts() {
            if let Some(secret) = credentials
                .load(&account.account.id)
                .map_err(crate::local_pool::accounts::credentials::credential_invalid_state_error)?
            {
                if account_proxy_pool_id(pool, &secret, common_proxy_id.as_deref()).as_deref()
                    == Some(proxy_id)
                {
                    affected_accounts.push(account.account.id.clone());
                }
            }
        }
    }
    pool.save()?;
    if let Some(runtime) = runtime {
        for account_id in affected_accounts {
            runtime
                .set_account_proxy_timezone(&account_id, timezone.clone())
                .map_err(LocalPoolError::invalid_state)?;
        }
    }
    state.oauth_events.emit_proxy_pool_changed();
    Ok(())
}
