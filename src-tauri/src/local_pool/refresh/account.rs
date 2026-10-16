use super::{DesktopState, RefreshRead, RefreshReadResult};
use crate::local_pool::{
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
    store::AccountRefreshFence,
};
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
};
use zenith_relay_core::scheduler::{
    account_member_key,
    refresh::{
        service::{RefreshRegistration, RefreshResult, RefreshWaitError},
        RefreshJob, RefreshKind, RefreshOutcome,
    },
};

pub(super) async fn reconcile(state: &DesktopState) -> Result<()> {
    let _mutation = state.setup_guard().await;
    let activity = super::active_members(state).await;
    let store = state.store()?;
    let mut active_fence_ids = BTreeSet::new();
    for account in store
        .accounts()
        .iter()
        .filter(|account| account.remote_location.is_none())
    {
        let (_, fence) = store.account_refresh_scope(&account.account.id)?;
        active_fence_ids.insert(fence.identity());
        for kind in [RefreshKind::Auth, RefreshKind::Quota, RefreshKind::Models] {
            register(state, account, fence.clone(), kind, true, &activity)?;
        }
    }
    super::sources::reconcile(state, &store, &activity, &mut active_fence_ids)?;
    state
        .refresh
        .retain(|identity, _| active_fence_ids.contains(identity));
    Ok(())
}

pub(crate) async fn request(
    state: &DesktopState,
    account_id: &str,
    kind: RefreshKind,
) -> RefreshReadResult {
    let fence = {
        let _mutation = state.setup_guard().await;
        let activity = super::active_members(state).await;
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
        .map_err(super::wait_error)?
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
        active: super::recently_used(account.account.last_used_at_ms)
            || activity.contains(&account_member_key(&account.account.id)),
        automatic: kind != RefreshKind::Auth
            && state.refresh_started.load(Ordering::Acquire)
            && state.background_session_active()
            && super::automatic_eligible(account),
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
                        refresh_value: Err(super::wait_error(RefreshWaitError::Stopped)),
                        outcome: RefreshOutcome::NoProgress,
                    };
                };
                let account_state = DesktopState { owner };
                let refresh_read = if job.kind == RefreshKind::Auth {
                    prepare_authorization(&account_state, &fence)
                        .await
                        .map(|prepared| RefreshRead::Authorization(Box::new(prepared)))
                } else {
                    execute(&account_state, &fence, &job).await
                };
                let outcome = match &refresh_read {
                    Ok(RefreshRead::Authorization(_)) => RefreshOutcome::Success,
                    Ok(RefreshRead::Quota(response))
                        if !matches!(response.quota, AccountQuotaOutcome::Failed { .. }) =>
                    {
                        RefreshOutcome::Success
                    }
                    Ok(RefreshRead::Models { succeeded: true }) => RefreshOutcome::Success,
                    _ if job.kind == RefreshKind::Auth => RefreshOutcome::NoProgress,
                    _ => RefreshOutcome::retry_after(account_state.refresh.now_ms(), None),
                };
                if let Err(error) = &refresh_read {
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
                RefreshResult {
                    refresh_value: refresh_read,
                    outcome,
                }
            })
        })
        .map_err(super::wait_error)
}

pub(super) async fn prepare_authorization(
    state: &DesktopState,
    fence: &AccountRefreshFence,
) -> Result<PreparedAccountAuthorization> {
    let scope = AccountRefreshScope::capture(state, &fence.account_id).await?;
    if scope.fence != *fence || scope.initial_account.remote_location.is_some() {
        return Err(super::wait_error(RefreshWaitError::Stale));
    }
    let prepared = prepare_account_request_authorization(state, &fence.account_id).await?;
    scope.validate(state)?;
    Ok(prepared)
}

pub(in crate::local_pool) async fn request_authorization(
    state: &DesktopState,
    fence: &AccountRefreshFence,
) -> Result<PreparedAccountAuthorization> {
    let authorization_result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Auth)
        .await
        .map_err(super::wait_error)?;
    match authorization_result.as_ref() {
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

pub(super) async fn execute(
    state: &DesktopState,
    fence: &AccountRefreshFence,
    job: &RefreshJob,
) -> RefreshReadResult {
    let scope = AccountRefreshScope::capture(state, &fence.account_id).await?;
    if scope.fence != *fence
        || scope.initial_account.remote_location.is_some()
        || (!job.manual
            && (!super::automatic_eligible(&scope.initial_account)
                || !state.background_session_active()))
    {
        return Err(super::wait_error(RefreshWaitError::Stale));
    }
    state.refresh.set_active(
        &job.identity,
        super::recently_used(scope.initial_account.account.last_used_at_ms)
            || super::active_members(state)
                .await
                .contains(&account_member_key(&fence.account_id)),
    );
    let refresh_read = match job.kind {
        RefreshKind::Quota => {
            let mut quota_response = read_account_quota_once(state, &scope, job.manual).await?;
            {
                let _mutation = state.setup_guard().await;
                scope.validate(state)?;
                super::automations::evaluate_updated_transitions(state, &quota_response)?;
            }
            // Reset verification stays inside this job. Enqueuing the same key
            // here would wait on itself, and followers must not settle twice.
            if super::automations::evaluate_weekly_exhaustions(state, &quota_response, fence)
                .await?
            {
                let next_scope = AccountRefreshScope::capture(state, &fence.account_id).await?;
                if next_scope.fence != *fence {
                    return Err(super::wait_error(RefreshWaitError::Stale));
                }
                quota_response = read_account_quota_once(state, &next_scope, false).await?;
            }
            if let Some(delay) = reset_due_delay(&quota_response, current_time_ms()) {
                state
                    .refresh
                    .schedule_after(&job.identity, RefreshKind::Quota, delay);
            }
            RefreshRead::Quota(Box::new(quota_response))
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
    let activity = super::active_members(state).await;
    let store = state.store()?;
    store.ensure_account_refresh_current(fence)?;
    if let Some(account) = store.account(&fence.account_id) {
        state.refresh.set_active(
            &job.identity,
            super::recently_used(account.account.last_used_at_ms)
                || activity.contains(&account_member_key(&fence.account_id)),
        );
    }
    Ok(refresh_read)
}

pub(in crate::local_pool) fn reset_due_delay(
    response: &AccountQuotaRefreshResponse,
    now_ms: u64,
) -> Option<u64> {
    if !matches!(response.quota, AccountQuotaOutcome::Updated { .. }) {
        return None;
    }
    zenith_relay_core::scheduler::refresh::quota_reset_delay(
        &response.account.account.id,
        &response.account.account.quota,
        now_ms,
    )
}
