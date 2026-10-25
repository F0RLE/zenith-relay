//! Proxy observations use the existing bounded refresh owner, never a request's
//! critical path or a renderer-owned polling loop.
use super::{wait_error, DesktopState, RefreshRead};
use crate::local_pool::{
    accounts::proxy::{
        check::{self, ProxyCheckResult},
        observations, ProxyPool, ProxyRefreshScope,
    },
    commands::current_time_ms,
    error::{LocalPoolError, Result},
};
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
};
use zenith_relay_core::{
    request_timezone::PROXY_TIMEZONE_MAX_AGE_MS,
    scheduler::refresh::{
        service::{RefreshRegistration, RefreshResult},
        RefreshIdentity, RefreshKind, RefreshOutcome,
    },
};

const PREFIX: &str = "proxy:";
const RETRY_MS: u64 = 60_000;

pub(super) async fn reconcile(state: &DesktopState) -> Result<()> {
    let _mutation = state.setup_guard().await;
    let mut active = BTreeSet::new();
    if state.background_session_active() {
        let credentials = crate::local_pool::accounts::credentials::CredentialStore::from_backend(
            crate::local_pool::accounts::NativeSecretBackend,
        );
        let pool = match observations::load_reconciled_pool(state, &credentials) {
            Ok(pool) => pool,
            Err(error) => {
                // One unavailable credential must not stop existing proxy jobs.
                crate::diagnostics::record_error(
                    "proxy-refresh",
                    Some("route_reconciliation_failed"),
                    &error.message,
                    &[],
                );
                ProxyPool::load()?
            }
        };
        for proxy in pool.refresh_scopes() {
            active.insert(register(state, proxy)?);
        }
    }
    state
        .refresh
        .retain(|identity, _| !is_proxy(identity) || active.contains(identity));
    Ok(())
}

pub(super) fn is_proxy(identity: &RefreshIdentity) -> bool {
    identity.member_id.starts_with(PREFIX)
}

fn register(state: &DesktopState, proxy: ProxyRefreshScope<'_>) -> Result<RefreshIdentity> {
    let identity = RefreshIdentity::new(format!("{PREFIX}{}", proxy.id), 0, proxy.created_at_ms);
    let expected_url = proxy.url.to_owned();
    let proxy_id = proxy.id.to_owned();
    let weak = Arc::downgrade(&state.owner);
    state
        .refresh
        .register(
            RefreshRegistration {
                identity: identity.clone(),
                kind: RefreshKind::Metadata,
                origin: "https://www.cloudflare.com".into(),
                active: false,
                automatic: state.refresh_started.load(Ordering::Acquire)
                    && state.background_session_active(),
                due_now: false,
            },
            move |job| {
                let (weak, proxy_id, expected_url) =
                    (weak.clone(), proxy_id.clone(), expected_url.clone());
                Box::pin(async move {
                    let Some(owner) = weak.upgrade() else {
                        return RefreshResult {
                            refresh_value: Err(LocalPoolError::invalid_state(
                                "refresh owner stopped",
                            )),
                            outcome: RefreshOutcome::NoProgress,
                        };
                    };
                    let state = DesktopState { owner };
                    let result = read(&state, &proxy_id, &expected_url, job.manual).await;
                    let outcome = if let Ok(result) = &result {
                        if result.error_code.is_none() && result.timezone.is_some() {
                            state.refresh.schedule_after(
                                &job.identity,
                                RefreshKind::Metadata,
                                next_delay(Some(result), current_time_ms()),
                            );
                            RefreshOutcome::Success
                        } else {
                            RefreshOutcome::retry_after(state.refresh.now_ms(), Some(RETRY_MS))
                        }
                    } else {
                        RefreshOutcome::retry_after(state.refresh.now_ms(), Some(RETRY_MS))
                    };
                    RefreshResult {
                        refresh_value: result.map(|result| RefreshRead::Proxy(Box::new(result))),
                        outcome,
                    }
                })
            },
        )
        .map_err(wait_error)?;
    if !state.refresh.in_flight(&identity, RefreshKind::Metadata) {
        state.refresh.schedule_after(
            &identity,
            RefreshKind::Metadata,
            next_delay(proxy.last_check, current_time_ms()),
        );
    }
    Ok(identity)
}

pub(crate) async fn request(state: &DesktopState, id: &str) -> Result<ProxyCheckResult> {
    let identity = {
        let _mutation = state.setup_guard().await;
        let pool = ProxyPool::load()?;
        let proxy = pool
            .refresh_scopes()
            .find(|proxy| proxy.id == id)
            .ok_or_else(|| LocalPoolError::invalid_state("stored proxy not found"))?;
        register(state, proxy)?
    };
    match state
        .refresh
        .request(&identity, RefreshKind::Metadata)
        .await
        .map_err(wait_error)?
        .as_ref()
    {
        Ok(RefreshRead::Proxy(result)) => Ok((**result).clone()),
        Err(error) => Err(error.clone()),
        _ => Err(LocalPoolError::invalid_state(
            "unexpected proxy refresh result",
        )),
    }
}

async fn read(
    state: &DesktopState,
    id: &str,
    expected_url: &str,
    manual: bool,
) -> Result<ProxyCheckResult> {
    let proxy = {
        let _mutation = state.setup_guard().await;
        if !manual && !state.background_session_active() {
            return Err(LocalPoolError::invalid_state(
                "background session is inactive",
            ));
        }
        let pool = ProxyPool::load()?;
        if pool.stored_url(id)? != expected_url {
            return Err(LocalPoolError::invalid_state("proxy changed during check"));
        }
        pool.config(id)?
    };
    let result = check::check(id.to_owned(), &proxy, current_time_ms()).await;
    let _mutation = state.setup_guard().await;
    let mut pool = ProxyPool::load()?;
    if !pool.record_check(expected_url, result.clone()) {
        return Err(LocalPoolError::invalid_state("proxy changed during check"));
    }
    observations::save_pool(state, &pool, id).await?;
    Ok(result)
}

fn next_delay(last: Option<&ProxyCheckResult>, now_ms: u64) -> u64 {
    let Some(last) = last else {
        return 0;
    };
    let interval = if last.error_code.is_none() && last.timezone.is_some() {
        // Refresh before expiry, leaving room for the bounded network probe.
        PROXY_TIMEZONE_MAX_AGE_MS - 30_000
    } else {
        RETRY_MS
    };
    last.checked_at_ms
        .checked_add(interval)
        .filter(|_| last.checked_at_ms <= now_ms)
        .map_or(0, |due| due.saturating_sub(now_ms))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_failed_or_unlocated_checks_retry_without_a_renderer() {
        assert_eq!(next_delay(None, 100_000), 0);
        let mut result = ProxyCheckResult {
            checked_at_ms: 100_000,
            ..Default::default()
        };
        assert_eq!(next_delay(Some(&result), 100_000), RETRY_MS);
        assert_eq!(next_delay(Some(&result), 160_000), 0);
        result.timezone = Some("Europe/Minsk".into());
        assert_eq!(
            next_delay(Some(&result), 100_000),
            PROXY_TIMEZONE_MAX_AGE_MS - 30_000
        );
        result.error_code = Some("proxy_check_timeout".into());
        assert_eq!(next_delay(Some(&result), 100_000), RETRY_MS);
        assert_eq!(next_delay(Some(&result), 99_999), 0);
    }
}
