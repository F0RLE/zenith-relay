use super::{
    current_time_ms, fence_runtime_candidates, restart_or_rollback, ProxyChoice,
    StoredProxyAssignmentResult,
};
use crate::local_pool::{
    accounts::{
        credentials::{
            credential_invalid_state_error as credential_error, CredentialStore,
            StoredCodexCredentials,
        },
        proxy::ProxyPool,
        NativeSecretBackend,
    },
    error::{ErrorCode, LocalPoolError, Result},
    state::DesktopState,
};
use std::collections::HashSet;

pub(crate) async fn set_account_proxy_inner(
    account_id: String,
    proxy_url: Option<String>,
    bypass_common_proxy: bool,
    state: &DesktopState,
) -> Result<StoredProxyAssignmentResult> {
    if proxy_url.is_some() && bypass_common_proxy {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "an account route cannot use and bypass a proxy at the same time",
        ));
    }
    let choice = proxy_url.map_or_else(
        || {
            if bypass_common_proxy {
                ProxyChoice::Direct
            } else {
                ProxyChoice::Inherited
            }
        },
        ProxyChoice::Custom,
    );
    apply_choices(state, vec![(account_id, choice)]).await
}

pub(super) async fn apply_choices(
    state: &DesktopState,
    choices: Vec<(String, ProxyChoice)>,
) -> Result<StoredProxyAssignmentResult> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let old_pool = load_reconciled_pool(state, &credentials)?;
    let mut pool = old_pool.clone();
    let account_ids = state
        .store()?
        .accounts()
        .iter()
        .map(|account| account.account.id.clone())
        .collect::<HashSet<_>>();
    let mut updates = Vec::new();
    let mut unchanged = 0;
    let mut unavailable = 0;
    for (account_id, choice) in choices {
        if !account_ids.contains(account_id.as_str()) {
            return Err(LocalPoolError::new(
                ErrorCode::NotFound,
                "account not found",
            ));
        }
        let previous_credentials = credentials.require(&account_id).map_err(credential_error)?;
        let (next_proxy_url, bypass_common_proxy) = match choice {
            ProxyChoice::Inherited => {
                pool.release(&account_id);
                (None, false)
            }
            ProxyChoice::Direct => {
                pool.release(&account_id);
                (None, true)
            }
            ProxyChoice::Automatic => match pool.assign_automatic(&account_id) {
                Some(url) => (Some(url), false),
                None => {
                    unavailable += 1;
                    continue;
                }
            },
            ProxyChoice::Stored(proxy_id) => (Some(pool.assign_id(&proxy_id, &account_id)?), false),
            ProxyChoice::Custom(proxy_url) => (
                Some(pool.assign_url(&proxy_url, &account_id, current_time_ms())?),
                false,
            ),
        };
        let updated_credentials = previous_credentials
            .clone()
            .with_proxy_route(next_proxy_url, bypass_common_proxy)
            .map_err(credential_error)?;
        if previous_credentials.proxy_url() == updated_credentials.proxy_url()
            && previous_credentials.bypass_common_proxy()
                == updated_credentials.bypass_common_proxy()
        {
            unchanged += 1;
        } else {
            updates.push((previous_credentials, updated_credentials));
        }
    }
    if updates.is_empty() && pool == old_pool {
        return Ok(StoredProxyAssignmentResult {
            assigned: 0,
            unchanged,
            unavailable,
            pool: pool.summary(),
        });
    }
    let affected_accounts = updates
        .iter()
        .map(|(_, updated_credentials)| updated_credentials.local_account_id().to_string())
        .collect::<Vec<_>>();
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences = fence_runtime_candidates(runtime.as_deref(), &affected_accounts, &[]);
    state.store()?.invalidate_account_refresh(
        &affected_accounts
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
    )?;
    save_credential_updates(&credentials, &updates)?;
    if let Err(error) = pool.save() {
        restore_credentials(&credentials, &updates)?;
        return Err(error);
    }
    let rollback_credentials = credentials.clone();
    let rollback_updates = updates.clone();
    restart_or_rollback(state, move || {
        restore_credentials(&rollback_credentials, &rollback_updates)?;
        old_pool.save()
    })
    .await?;
    let now_ms = current_time_ms();
    for (_, updated_credentials) in &updates {
        state.sync_account_quota_refresh(updated_credentials.local_account_id(), now_ms)?;
    }
    Ok(StoredProxyAssignmentResult {
        assigned: updates.len(),
        unchanged,
        unavailable,
        pool: pool.summary(),
    })
}

pub(super) fn load_reconciled_pool(
    state: &DesktopState,
    credentials: &CredentialStore<NativeSecretBackend>,
) -> Result<ProxyPool> {
    let mut account_proxies = Vec::new();
    for account in state.store()?.accounts() {
        let proxy = credentials
            .load(&account.account.id)
            .map_err(credential_error)?
            .and_then(|stored| stored.proxy_url().map(str::to_string));
        account_proxies.push((account.account.id.clone(), proxy));
    }
    account_proxies.sort_by(|left, right| left.0.cmp(&right.0));
    let mut pool = ProxyPool::load()?;
    if pool.reconcile(&account_proxies, current_time_ms()) {
        pool.save()?;
    }
    Ok(pool)
}

fn save_credential_updates(
    credentials: &CredentialStore<NativeSecretBackend>,
    updates: &[(StoredCodexCredentials, StoredCodexCredentials)],
) -> Result<()> {
    for index in 0..updates.len() {
        if let Err(error) = credentials
            .save(&updates[index].1)
            .map_err(credential_error)
        {
            restore_credentials(credentials, &updates[..index])?;
            return Err(error);
        }
    }
    Ok(())
}

fn restore_credentials(
    credentials: &CredentialStore<NativeSecretBackend>,
    updates: &[(StoredCodexCredentials, StoredCodexCredentials)],
) -> Result<()> {
    for (previous_credentials, _) in updates {
        credentials
            .save(previous_credentials)
            .map_err(credential_error)?;
    }
    Ok(())
}

pub(super) fn normalize_ids(proxy_urls: Vec<String>, allow_empty: bool) -> Result<Vec<String>> {
    let mut seen = HashSet::new();
    let normalized_proxy_urls = proxy_urls
        .into_iter()
        .map(|proxy_url| proxy_url.trim().to_string())
        .filter(|proxy_url| !proxy_url.is_empty())
        .collect::<Vec<_>>();
    if (!allow_empty && normalized_proxy_urls.is_empty())
        || normalized_proxy_urls
            .iter()
            .any(|proxy_url| !seen.insert(proxy_url.clone()))
    {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "selection is empty or contains duplicates",
        ));
    }
    Ok(normalized_proxy_urls)
}
