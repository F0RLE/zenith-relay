//! Server adapter for the shared refresh service. Startup, operator requests
//! and wake verification all use these same quota/model jobs. Provider parsing
//! and OAuth exchange remain in their existing owners.

mod requests;
mod runtime;
mod sources;
pub(crate) use requests::{refresh_account_now, refresh_all_accounts_now};
pub(crate) use sources::{
    cached_stats as cached_source_stats, request_models as request_source_models,
    request_stats as request_source_stats,
};

use super::{account_models, quota_refresh, wake_automation, weekly_reset};
use crate::{
    app::prepare_server_account_authorization,
    state::{now_ms, AccountCredential, AppState, ServerAccountRecord, SourceRecord},
    store::AccountRefreshFence,
};
use reqwest::header::HeaderValue;
use std::{collections::BTreeSet, sync::Arc};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::{
    accounts::automatic_quota_monitoring_eligible,
    quota::QuotaTransition,
    scheduler::refresh::{
        http::ManagementHttpScope,
        quota_reset_delay,
        service::{RefreshRegistration, RefreshResult},
        RefreshJob, RefreshKind, RefreshOutcome,
    },
};

pub(super) fn account_http_scope(
    state: &Arc<AppState>,
    fence: &AccountRefreshFence,
) -> ManagementHttpScope {
    let state = state.clone();
    let fence = fence.clone();
    ManagementHttpScope::checked(move || {
        state
            .store
            .account_refresh_scope(&fence.account_id)
            .is_ok_and(|(_, current)| current == fence)
    })
}

#[derive(Clone)]
pub(crate) struct AccountRead {
    pub account: ServerAccountRecord,
    pub transitions: Vec<QuotaTransition>,
    pub succeeded: bool,
    pub retry_after_ms: Option<u64>,
    pub models_changed: bool,
    /// Only a durable auth/health transition may lift a live runtime block.
    pub health_changed: bool,
}

#[derive(Clone)]
pub(crate) enum RefreshRead {
    Account(Box<AccountRead>),
    /// Transient secret material for waiting jobs only; never cache or expose.
    Authorization(Result<Box<PreparedAuthorization>, AuthorizationFailure>),
    SourceModels(Box<SourceRecord>),
    SourceStats(zenith_relay_core::scheduler::refresh::SourceStatsObservation),
}

pub(crate) type RefreshReadResult = Result<RefreshRead, String>;

#[derive(Clone)]
pub(crate) struct PreparedAuthorization {
    pub credential: AccountCredential,
    pub header: HeaderValue,
    /// Only a bearer authorization has exact token evidence for a 401 CAS.
    pub oauth_tokens: Option<zenith_relay_core::accounts::TokenSet>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AuthorizationFailure {
    SecretLoad,
    SecretMissing,
    SecretInvalid,
    Prepare,
    Stale,
}

pub(crate) fn cache_observation(value: &RefreshReadResult) -> bool {
    matches!(value, Ok(read) if !matches!(read, RefreshRead::Authorization(_)))
}

pub(super) fn start(state: Arc<AppState>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut changes = state.store.refresh_changes();
        loop {
            if *shutdown.borrow() {
                break;
            }
            // Subscribe before reconciliation so an edit during the scan cannot
            // leave a stale registration asleep until its old periodic timer.
            changes.borrow_and_update();
            let _ = reconcile(&state);
            tokio::select! {
                changed = shutdown.changed() => { if changed.is_err() || *shutdown.borrow() { break; } }
                changed = changes.changed() => { if changed.is_err() { break; } }
            }
        }
        state.refresh.shutdown().await;
    })
}

fn reconcile(state: &Arc<AppState>) -> Result<(), String> {
    let accounts = state.store.accounts()?;
    let activity = active_runtime_members(state)?;
    let mut current_ids = BTreeSet::new();
    for account in accounts {
        let (account, fence) = state.store.account_refresh_scope(&account.id)?;
        let active = recently_used(account.last_used_at_ms)
            || activity.contains(&format!("account:{}", account.id));
        current_ids.insert(fence.identity());
        for kind in [RefreshKind::Auth, RefreshKind::Quota, RefreshKind::Models] {
            register(state, &account, fence.clone(), kind, true, active)?;
        }
    }
    sources::reconcile(state, &activity, &mut current_ids)?;
    state
        .refresh
        .retain(|identity, _| current_ids.contains(identity));
    Ok(())
}

pub(super) async fn request(
    state: &Arc<AppState>,
    account_id: &str,
    kind: RefreshKind,
) -> Result<AccountRead, String> {
    let (account, fence) = state.store.account_refresh_scope(account_id)?;
    register(
        state,
        &account,
        fence.clone(),
        RefreshKind::Auth,
        false,
        is_active(state, &account)?,
    )?;
    register(
        state,
        &account,
        fence.clone(),
        kind,
        false,
        is_active(state, &account)?,
    )?;
    let result = state
        .refresh
        .request(&fence.identity(), kind)
        .await
        .map_err(|_| "account refresh could not complete".to_string())?
        .as_ref()
        .clone()?;
    let (_, current) = state.store.account_refresh_scope(account_id)?;
    if current != fence {
        return Err("account changed during refresh".into());
    }
    match result {
        RefreshRead::Account(read) => Ok(*read),
        _ => Err("unexpected account refresh result".into()),
    }
}

fn register(
    state: &Arc<AppState>,
    account: &ServerAccountRecord,
    fence: AccountRefreshFence,
    kind: RefreshKind,
    due_now: bool,
    active: bool,
) -> Result<(), String> {
    let registration = RefreshRegistration {
        identity: fence.identity(),
        kind,
        origin: "https://chatgpt.com".into(),
        active,
        automatic: kind != RefreshKind::Auth
            && automatic_quota_monitoring_eligible(account.enabled, account.auth_state),
        due_now,
    };
    let weak = Arc::downgrade(state);
    state
        .refresh
        .register(registration, move |job| {
            let (weak, fence) = (weak.clone(), fence.clone());
            Box::pin(async move {
                let Some(state) = weak.upgrade() else {
                    return RefreshResult {
                        value: Err("refresh owner stopped".into()),
                        outcome: RefreshOutcome::NoProgress,
                    };
                };
                let value = if job.kind == RefreshKind::Auth {
                    Ok(RefreshRead::Authorization(
                        prepare_authorization(&state, &fence).await.map(Box::new),
                    ))
                } else {
                    execute(&state, &fence, &job)
                        .await
                        .map(|read| RefreshRead::Account(Box::new(read)))
                };
                let outcome = match &value {
                    Ok(RefreshRead::Authorization(Ok(_))) => RefreshOutcome::Success,
                    Ok(RefreshRead::Authorization(Err(_))) => RefreshOutcome::NoProgress,
                    Ok(RefreshRead::Account(read)) if read.succeeded => RefreshOutcome::Success,
                    _ if job.kind == RefreshKind::Auth => RefreshOutcome::NoProgress,
                    _ => {
                        let delay = match &value {
                            Ok(RefreshRead::Account(read)) => read.retry_after_ms,
                            _ => None,
                        }
                        .unwrap_or_default()
                        .max(60_000);
                        RefreshOutcome::FailedRetryAt(state.refresh.now_ms().saturating_add(delay))
                    }
                };
                RefreshResult { value, outcome }
            })
        })
        .map_err(|_| "account refresh could not be scheduled".to_string())
}

async fn prepare_authorization(
    state: &Arc<AppState>,
    fence: &AccountRefreshFence,
) -> Result<PreparedAuthorization, AuthorizationFailure> {
    let (account, current) = state
        .store
        .account_refresh_scope(&fence.account_id)
        .map_err(|_| AuthorizationFailure::Stale)?;
    if &current != fence {
        return Err(AuthorizationFailure::Stale);
    }
    let secret = state
        .vault
        .load(&account.secret_ref)
        .map_err(|_| AuthorizationFailure::SecretLoad)?
        .ok_or(AuthorizationFailure::SecretMissing)?;
    let credential: AccountCredential =
        serde_json::from_str(&secret).map_err(|_| AuthorizationFailure::SecretInvalid)?;
    let (credential, header, oauth_tokens) =
        prepare_server_account_authorization(state, &account, credential, None)
            .await
            .map_err(|_| AuthorizationFailure::Prepare)?;
    let (_, current) = state
        .store
        .account_refresh_scope(&fence.account_id)
        .map_err(|_| AuthorizationFailure::Stale)?;
    if &current != fence {
        return Err(AuthorizationFailure::Stale);
    }
    Ok(PreparedAuthorization {
        credential,
        header,
        oauth_tokens,
    })
}

pub(super) async fn request_authorization(
    state: &Arc<AppState>,
    fence: &AccountRefreshFence,
) -> Result<PreparedAuthorization, AuthorizationFailure> {
    let result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Auth)
        .await
        .map_err(|error| match error {
            zenith_relay_core::scheduler::refresh::service::RefreshWaitError::Stale => {
                AuthorizationFailure::Stale
            }
            _ => AuthorizationFailure::Prepare,
        })?;
    let prepared = match result.as_ref() {
        Ok(RefreshRead::Authorization(Ok(prepared))) => Ok((**prepared).clone()),
        Ok(RefreshRead::Authorization(Err(error))) => Err(*error),
        _ => Err(AuthorizationFailure::Prepare),
    }?;
    // The shared read could have finished just before an operator changed the
    // login/proxy. Do not start provider HTTP with that obsolete preparation.
    let (_, current) = state
        .store
        .account_refresh_scope(&fence.account_id)
        .map_err(|_| AuthorizationFailure::Stale)?;
    if &current != fence {
        return Err(AuthorizationFailure::Stale);
    }
    Ok(prepared)
}

fn is_active(state: &AppState, account: &ServerAccountRecord) -> Result<bool, String> {
    Ok(recently_used(account.last_used_at_ms)
        || active_runtime_members(state)?.contains(&format!("account:{}", account.id)))
}

fn active_runtime_members(state: &AppState) -> Result<BTreeSet<String>, String> {
    Ok(state
        .runtime()?
        .map(|runtime| runtime.active_member_keys(now_ms(), 10 * 60_000))
        .unwrap_or_default())
}

fn recently_used(last_used: Option<u64>) -> bool {
    last_used.is_some_and(|at| at <= now_ms() && now_ms().saturating_sub(at) < 10 * 60_000)
}

async fn execute(
    state: &Arc<AppState>,
    fence: &AccountRefreshFence,
    job: &RefreshJob,
) -> Result<AccountRead, String> {
    let (account, current) = state.store.account_refresh_scope(&fence.account_id)?;
    if &current != fence
        || (!job.manual
            && !automatic_quota_monitoring_eligible(account.enabled, account.auth_state))
    {
        return Err("account changed during refresh".into());
    }
    state
        .refresh
        .set_active(&job.identity, is_active(state, &account)?);
    let mut read = match job.kind {
        RefreshKind::Quota => quota_refresh::read_one(state, fence, job.manual).await?,
        RefreshKind::Models => account_models::read_models(state, fence).await?,
        _ => return Err("unsupported account refresh kind".into()),
    };
    if job.kind == RefreshKind::Quota && !read.transitions.is_empty() {
        // Reset verification remains inside this same quota job; recursively
        // requesting the single-flight key here would deadlock on itself.
        if weekly_reset::try_auto_reset_weekly(state, fence, &read.account, &read.transitions)
            .await
            .unwrap_or(false)
        {
            let refreshed = quota_refresh::read_one(state, fence, false).await?;
            read.account = refreshed.account;
            read.succeeded = refreshed.succeeded;
            read.retry_after_ms = refreshed.retry_after_ms;
        }
        if read.succeeded {
            wake_automation::schedule_transitions(state, &read.account, &read.transitions).await?;
        }
    }
    runtime::synchronize(state, fence, read.models_changed, read.health_changed).await?;
    if job.kind == RefreshKind::Quota {
        schedule_quota_reset(state, fence, &read);
    }
    Ok(read)
}

fn schedule_quota_reset(state: &AppState, fence: &AccountRefreshFence, read: &AccountRead) {
    if read.succeeded {
        if let Some(delay) = quota_reset_delay(&read.account.id, &read.account.quota, now_ms()) {
            state
                .refresh
                .schedule_after(&fence.identity(), RefreshKind::Quota, delay);
        }
    }
}

#[cfg(test)]
mod tests;
