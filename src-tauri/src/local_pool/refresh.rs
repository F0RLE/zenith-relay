//! Desktop adapter for the shared account and source refresh lifecycle. The store
//! supplies revision/eligibility events; provider readers only apply fenced
//! observations. No queue snapshots, per-caller settlement or polling scans.

mod automations;
pub(crate) mod sources;

use super::{
    accounts::{
        quota_refresh::{
            prepare_account_request_authorization, read_account_models_once,
            read_account_quota_once, AccountQuotaOutcome, AccountQuotaRefreshResponse,
            PreparedAccountAuthorization,
        },
        refresh_observations::AccountRefreshScope,
    },
    commands::current_time_ms,
    error::{ErrorCode, LocalPoolError, Result},
    models::LocalAccountRecord,
    state::DesktopState,
    store::AccountRefreshFence,
};
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
};
use zenith_relay_core::scheduler::refresh::{
    service::{RefreshRegistration, RefreshResult, RefreshWaitError},
    RefreshJob, RefreshKind, RefreshOutcome,
};

#[derive(Clone, Debug)]
pub(crate) enum RefreshRead {
    Quota(Box<AccountQuotaRefreshResponse>),
    Models {
        succeeded: bool,
    },
    /// Only shared with current waiters; credentials never enter observation cache.
    Authorization(Box<PreparedAccountAuthorization>),
    SourceModels(Box<super::models::ProviderSourceRecord>),
    SourceStats(zenith_relay_core::scheduler::refresh::SourceStatsObservation),
}

pub(crate) type RefreshReadResult = Result<RefreshRead>;

pub(crate) fn cache_observation(value: &RefreshReadResult) -> bool {
    matches!(value, Ok(read) if !matches!(read, RefreshRead::Authorization(_)))
}

impl DesktopState {
    /// Opening storage alone must never start provider work (including tests
    /// and recovery tools). The native application explicitly starts this owner.
    pub(crate) fn start_refresh(&self) -> Result<()> {
        let mut changes = self.store()?.refresh_changes();
        if self.refresh_started.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        let weak = Arc::downgrade(&self.owner);
        let event_owner = weak.clone();
        let mut progress = self.refresh.progress();
        tauri::async_runtime::spawn(async move {
            while progress.changed().await.is_ok() {
                let Some(owner) = event_owner.upgrade() else {
                    break;
                };
                owner.oauth_events.emit_state_changed();
            }
        });
        tauri::async_runtime::spawn(async move {
            loop {
                changes.borrow_and_update();
                let Some(owner) = weak.upgrade() else {
                    break;
                };
                let state = DesktopState { owner };
                if let Err(error) = reconcile(&state).await {
                    crate::diagnostics::record_error(
                        "refresh",
                        Some("reconcile_failed"),
                        &error.message,
                        &[],
                    );
                }
                // Never retain the host while idle, including via a callback.
                drop(state);
                if changes.changed().await.is_err() {
                    break;
                }
            }
        });
        Ok(())
    }

    /// A focused host event, not another queue. Newly eligible/revived accounts
    /// are registered by the store watcher after the setup transaction finishes.
    pub(crate) fn sync_account_quota_refresh(
        &self,
        account_id: &str,
        due_at_ms: u64,
    ) -> Result<bool> {
        let store = self.store()?;
        let Some(account) = store.account(account_id) else {
            return Ok(self.refresh.remove_member(&format!("account:{account_id}")));
        };
        let eligible = automatic_eligible(account);
        if eligible {
            let (_, fence) = store.account_refresh_scope(account_id)?;
            self.refresh.schedule_after(
                &fence.identity(),
                RefreshKind::Quota,
                due_at_ms.saturating_sub(current_time_ms()),
            );
        }
        store.notify_refresh_changed();
        Ok(eligible)
    }

    pub(crate) fn remove_account_refresh(&self, account_id: &str) -> bool {
        self.refresh.remove_member(&format!("account:{account_id}"))
    }

    pub(crate) fn quota_refresh_in_flight(&self, account_id: &str) -> Result<bool> {
        let store = self.store()?;
        if store.account(account_id).is_none() {
            return Ok(false);
        }
        let (_, fence) = store.account_refresh_scope(account_id)?;
        Ok(self
            .refresh
            .in_flight(&fence.identity(), RefreshKind::Quota))
    }
}

async fn reconcile(state: &DesktopState) -> Result<()> {
    let _mutation = state.setup_guard().await;
    let activity = active_members(state).await;
    let store = state.store()?;
    let mut current = BTreeSet::new();
    for account in store
        .accounts()
        .iter()
        .filter(|account| account.remote_location.is_none())
    {
        let (_, fence) = store.account_refresh_scope(&account.account.id)?;
        current.insert(fence.identity());
        for kind in [RefreshKind::Auth, RefreshKind::Quota, RefreshKind::Models] {
            register(state, account, fence.clone(), kind, true, &activity)?;
        }
    }
    sources::reconcile(state, &store, &activity, &mut current)?;
    state
        .refresh
        .retain(|identity, _| current.contains(identity));
    Ok(())
}

pub(crate) async fn request(
    state: &DesktopState,
    account_id: &str,
    kind: RefreshKind,
) -> RefreshReadResult {
    let fence = {
        let _mutation = state.setup_guard().await;
        let activity = active_members(state).await;
        let store = state.store()?;
        let (account, fence) = store.account_refresh_scope(account_id)?;
        if account.remote_location.is_some() {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "account is managed by a remote server",
            ));
        }
        if kind != RefreshKind::Auth {
            register(
                state,
                &account,
                fence.clone(),
                RefreshKind::Auth,
                false,
                &activity,
            )?;
        }
        register(state, &account, fence.clone(), kind, false, &activity)?;
        fence
    };
    state
        .refresh
        .request(&fence.identity(), kind)
        .await
        .map_err(wait_error)?
        .as_ref()
        .clone()
}

fn register(
    state: &DesktopState,
    account: &LocalAccountRecord,
    fence: AccountRefreshFence,
    kind: RefreshKind,
    due_now: bool,
    activity: &BTreeSet<String>,
) -> Result<()> {
    let registration = RefreshRegistration {
        identity: fence.identity(),
        kind,
        origin: "https://chatgpt.com".into(),
        active: recently_used(account.account.last_used_at_ms)
            || activity.contains(&format!("account:{}", account.account.id)),
        automatic: kind != RefreshKind::Auth
            && state.refresh_started.load(Ordering::Acquire)
            && state.background_session_active()
            && automatic_eligible(account),
        due_now,
    };
    let weak = Arc::downgrade(&state.owner);
    state
        .refresh
        .register(registration, move |job| {
            let (weak, fence) = (weak.clone(), fence.clone());
            Box::pin(async move {
                let Some(owner) = weak.upgrade() else {
                    return RefreshResult {
                        value: Err(wait_error(RefreshWaitError::Stopped)),
                        outcome: RefreshOutcome::NoProgress,
                    };
                };
                let state = DesktopState { owner };
                let value = if job.kind == RefreshKind::Auth {
                    prepare_authorization(&state, &fence)
                        .await
                        .map(|prepared| RefreshRead::Authorization(Box::new(prepared)))
                } else {
                    execute(&state, &fence, &job).await
                };
                let outcome = match &value {
                    Ok(RefreshRead::Authorization(_)) => RefreshOutcome::Success,
                    Ok(RefreshRead::Quota(response))
                        if !matches!(response.quota, AccountQuotaOutcome::Failed { .. }) =>
                    {
                        RefreshOutcome::Success
                    }
                    Ok(RefreshRead::Models { succeeded: true }) => RefreshOutcome::Success,
                    _ if job.kind == RefreshKind::Auth => RefreshOutcome::NoProgress,
                    _ => {
                        RefreshOutcome::FailedRetryAt(state.refresh.now_ms().saturating_add(60_000))
                    }
                };
                if let Err(error) = &value {
                    crate::diagnostics::record_error(
                        "account-refresh",
                        Some("refresh_failed"),
                        &error.message,
                        &[(
                            "account",
                            crate::diagnostics::hash_identifier(&fence.account_id),
                        )],
                    );
                }
                RefreshResult { value, outcome }
            })
        })
        .map_err(wait_error)
}

async fn prepare_authorization(
    state: &DesktopState,
    fence: &AccountRefreshFence,
) -> Result<PreparedAccountAuthorization> {
    let scope = AccountRefreshScope::capture(state, &fence.account_id).await?;
    if scope.fence != *fence || scope.before.remote_location.is_some() {
        return Err(wait_error(RefreshWaitError::Stale));
    }
    let prepared = prepare_account_request_authorization(state, &fence.account_id).await?;
    scope.validate(state)?;
    Ok(prepared)
}

pub(in crate::local_pool) async fn request_authorization(
    state: &DesktopState,
    fence: &AccountRefreshFence,
) -> Result<PreparedAccountAuthorization> {
    let result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Auth)
        .await
        .map_err(wait_error)?;
    match result.as_ref() {
        Ok(RefreshRead::Authorization(prepared)) => Ok((**prepared).clone()),
        Ok(_) => Err(LocalPoolError::invalid_state(
            "unexpected authorization result",
        )),
        Err(error) => Err(error.clone()),
    }
}

/// A reset-credit command may run without an account refresh job. Register
/// only its Auth prerequisite, and retain the caller's original ownership fence.
pub(in crate::local_pool) async fn request_authorization_now(
    state: &DesktopState,
    fence: &AccountRefreshFence,
) -> Result<PreparedAccountAuthorization> {
    state.store()?.ensure_account_refresh_current(fence)?;
    let prepared = match request(state, &fence.account_id, RefreshKind::Auth).await? {
        RefreshRead::Authorization(prepared) => *prepared,
        _ => {
            return Err(LocalPoolError::invalid_state(
                "unexpected authorization result",
            ))
        }
    };
    state.store()?.ensure_account_refresh_current(fence)?;
    Ok(prepared)
}

async fn execute(
    state: &DesktopState,
    fence: &AccountRefreshFence,
    job: &RefreshJob,
) -> RefreshReadResult {
    let scope = AccountRefreshScope::capture(state, &fence.account_id).await?;
    if scope.fence != *fence
        || scope.before.remote_location.is_some()
        || (!job.manual
            && (!automatic_eligible(&scope.before) || !state.background_session_active()))
    {
        return Err(wait_error(RefreshWaitError::Stale));
    }
    state.refresh.set_active(
        &job.identity,
        recently_used(scope.before.account.last_used_at_ms)
            || active_members(state).await.contains(&fence.account_id),
    );
    let read = match job.kind {
        RefreshKind::Quota => {
            let mut response = read_account_quota_once(state, &scope, job.manual).await?;
            {
                let _mutation = state.setup_guard().await;
                scope.validate(state)?;
                automations::evaluate_updated_transitions(state, &response)?;
            }
            // Reset verification stays inside this job. Enqueuing the same key
            // here would wait on itself, and followers must not settle twice.
            if automations::evaluate_weekly_exhaustions(state, &response, fence).await? {
                let next_scope = AccountRefreshScope::capture(state, &fence.account_id).await?;
                if next_scope.fence != *fence {
                    return Err(wait_error(RefreshWaitError::Stale));
                }
                response = read_account_quota_once(state, &next_scope, false).await?;
            }
            if let Some(delay) = reset_due_delay(&response, current_time_ms()) {
                state
                    .refresh
                    .schedule_after(&job.identity, RefreshKind::Quota, delay);
            }
            RefreshRead::Quota(Box::new(response))
        }
        RefreshKind::Models => RefreshRead::Models {
            succeeded: read_account_models_once(state, &scope).await?,
        },
        _ => {
            return Err(LocalPoolError::invalid_state(
                "unsupported account refresh kind",
            ))
        }
    };
    let activity = active_members(state).await;
    let store = state.store()?;
    store.ensure_account_refresh_current(fence)?;
    if let Some(account) = store.account(&fence.account_id) {
        state.refresh.set_active(
            &job.identity,
            recently_used(account.account.last_used_at_ms)
                || activity.contains(&format!("account:{}", fence.account_id)),
        );
    }
    Ok(read)
}

fn automatic_eligible(account: &LocalAccountRecord) -> bool {
    account.remote_location.is_none() && account.account.is_automatic_quota_monitoring_eligible()
}

async fn active_members(state: &DesktopState) -> BTreeSet<String> {
    state
        .gateway
        .runtime()
        .await
        .map(|runtime| runtime.active_member_keys(current_time_ms(), 10 * 60_000))
        .unwrap_or_default()
}

fn recently_used(at: Option<u64>) -> bool {
    let now = current_time_ms();
    at.is_some_and(|at| at <= now && now.saturating_sub(at) < 10 * 60_000)
}

pub(super) fn reset_due_delay(response: &AccountQuotaRefreshResponse, now_ms: u64) -> Option<u64> {
    if !matches!(response.quota, AccountQuotaOutcome::Updated { .. }) {
        return None;
    }
    zenith_relay_core::scheduler::refresh::quota_reset_delay(
        &response.account.account.id,
        &response.account.account.quota,
        now_ms,
    )
}

fn wait_error(error: RefreshWaitError) -> LocalPoolError {
    let (code, message) = match error {
        RefreshWaitError::Stale => (ErrorCode::Conflict, "connection changed during refresh"),
        RefreshWaitError::Full => (ErrorCode::InvalidState, "refresh capacity is full"),
        RefreshWaitError::Stopped | RefreshWaitError::Interrupted => {
            (ErrorCode::InvalidState, "refresh was interrupted")
        }
    };
    LocalPoolError::new(code, message)
}

#[cfg(test)]
mod tests;
