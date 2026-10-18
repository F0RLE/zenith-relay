use super::*;
use crate::local_pool::error::{LocalPoolError, Result};

/// Called only under the setup mutation guard. Resolve credential routes before
/// committing so a failed read cannot leave storage and the runtime out of sync.
pub(super) async fn save_pool(
    state: &DesktopState,
    pool: &ProxyPool,
    proxy_id: &str,
) -> Result<()> {
    let timezone = pool.timezone(proxy_id)?;
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
    Ok(())
}
