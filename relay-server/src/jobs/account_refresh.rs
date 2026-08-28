use super::{quota_refresh, wake_automation};
use crate::state::{AppState, ServerAccountRecord};
use futures_util::{stream, StreamExt};
use std::sync::Arc;
use zenith_relay_core::accounts::automatic_quota_monitoring_eligible;

const QUOTA_REFRESH_BATCH_SIZE: usize = 5;

#[derive(Clone, Copy)]
enum RefreshInvocation {
    Manual,
    BackgroundBatch,
}

impl RefreshInvocation {
    fn reset_failure_policy(self) -> ResetRefreshFailurePolicy {
        match self {
            Self::Manual => ResetRefreshFailurePolicy::Propagate,
            Self::BackgroundBatch => ResetRefreshFailurePolicy::KeepCurrentAccount,
        }
    }

    fn skip_scheduling_after_quota_error(self) -> bool {
        matches!(self, Self::BackgroundBatch)
    }
}

#[derive(Clone, Copy)]
enum ResetRefreshFailurePolicy {
    Propagate,
    KeepCurrentAccount,
}

pub(crate) async fn refresh_account_now(
    state: &Arc<AppState>,
    account: ServerAccountRecord,
) -> Result<ServerAccountRecord, String> {
    let (mut updated, transitions) =
        quota_refresh::refresh_account_metadata(state, account, true, true).await?;
    updated = finalize_refresh(state, updated, &transitions, RefreshInvocation::Manual).await?;
    let _ = state.rebuild_runtime().await;
    Ok(updated)
}

pub(crate) async fn refresh_all_accounts_now(
    state: &Arc<AppState>,
) -> Result<(usize, usize), String> {
    let accounts = state.store.accounts()?;
    refresh_accounts_now(state, accounts, true, true).await
}

pub(crate) async fn refresh_automatic_accounts_now(
    state: &Arc<AppState>,
    refresh_models: bool,
) -> Result<(usize, usize), String> {
    let accounts = state
        .store
        .accounts()?
        .into_iter()
        .filter(|account| automatic_quota_monitoring_eligible(account.enabled, account.auth_state))
        .collect::<Vec<_>>();
    refresh_accounts_now(state, accounts, false, refresh_models).await
}

async fn refresh_accounts_now(
    state: &Arc<AppState>,
    accounts: Vec<ServerAccountRecord>,
    force_subscription_refresh: bool,
    refresh_models: bool,
) -> Result<(usize, usize), String> {
    let results = stream::iter(accounts.into_iter().map(|account| {
        let state = Arc::clone(state);
        async move {
            quota_refresh::refresh_account_metadata(
                &state,
                account,
                force_subscription_refresh,
                refresh_models,
            )
            .await
        }
    }))
    .buffer_unordered(QUOTA_REFRESH_BATCH_SIZE)
    .collect::<Vec<_>>()
    .await;

    let mut refreshed = 0;
    let mut failed = 0;
    for result in results {
        let Ok((updated, transitions)) = result else {
            failed += 1;
            continue;
        };
        let Ok(updated) = finalize_refresh(
            state,
            updated,
            &transitions,
            RefreshInvocation::BackgroundBatch,
        )
        .await
        else {
            failed += 1;
            continue;
        };
        if updated.quota.error.is_some() {
            failed += 1;
            continue;
        }
        refreshed += 1;
    }
    state.rebuild_runtime().await?;
    Ok((refreshed, failed))
}

async fn finalize_refresh(
    state: &Arc<AppState>,
    mut account: ServerAccountRecord,
    transitions: &[zenith_relay_core::quota::QuotaTransition],
    invocation: RefreshInvocation,
) -> Result<ServerAccountRecord, String> {
    if transitions.is_empty() {
        return Ok(account);
    }

    let reset_applied = quota_refresh::try_auto_reset_weekly(state, &account, transitions)
        .await
        .unwrap_or(false);
    if reset_applied {
        match quota_refresh::refresh_one(state, account.clone(), false).await {
            Ok((refreshed, _)) => account = refreshed,
            Err(error)
                if matches!(
                    invocation.reset_failure_policy(),
                    ResetRefreshFailurePolicy::Propagate
                ) =>
            {
                return Err(error);
            }
            Err(_) => {}
        }
    }

    if invocation.skip_scheduling_after_quota_error() && account.quota.error.is_some() {
        return Ok(account);
    }
    wake_automation::schedule_transitions(state, &account, transitions).await?;
    Ok(account)
}
