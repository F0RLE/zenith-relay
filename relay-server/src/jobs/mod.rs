mod health_probe;
pub(crate) mod quota_refresh;
mod wake_automation;

use crate::state::{AppState, ServerAccountRecord};
use futures_util::{stream, StreamExt};
use std::sync::Arc;
use zenith_relay_core::accounts::automatic_quota_refresh_eligible;

const QUOTA_REFRESH_BATCH_SIZE: usize = 5;

pub fn start(state: Arc<AppState>) {
    health_probe::start(state.clone());
    quota_refresh::start(state.clone());
    wake_automation::start(state);
}

pub(crate) async fn refresh_account_now(
    state: &Arc<AppState>,
    account: ServerAccountRecord,
) -> Result<ServerAccountRecord, String> {
    let (updated, transitions) =
        quota_refresh::refresh_account_metadata(state, account, true, true).await?;
    if !transitions.is_empty() {
        wake_automation::schedule_transitions(state, &updated, &transitions).await?;
    }
    let _ = state.rebuild_runtime().await;
    Ok(updated)
}

pub(crate) async fn refresh_all_accounts_now(
    state: &Arc<AppState>,
) -> Result<(usize, usize), String> {
    let accounts = state.store.accounts()?;
    refresh_accounts_now(state, accounts, true).await
}

pub(crate) async fn refresh_automatic_accounts_now(
    state: &Arc<AppState>,
) -> Result<(usize, usize), String> {
    let accounts = state
        .store
        .accounts()?
        .into_iter()
        .filter(|account| {
            automatic_quota_refresh_eligible(
                account.enabled,
                account.in_pool,
                account.draining,
                account.auth_state,
                account.health,
            )
        })
        .collect::<Vec<_>>();
    refresh_accounts_now(state, accounts, false).await
}

async fn refresh_accounts_now(
    state: &Arc<AppState>,
    accounts: Vec<ServerAccountRecord>,
    force_subscription_refresh: bool,
) -> Result<(usize, usize), String> {
    let results = stream::iter(accounts.into_iter().map(|account| {
        let state = Arc::clone(state);
        async move {
            quota_refresh::refresh_account_metadata(
                &state,
                account,
                force_subscription_refresh,
                false,
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
        if !transitions.is_empty()
            && wake_automation::schedule_transitions(state, &updated, &transitions)
                .await
                .is_err()
        {
            failed += 1;
            continue;
        }
        refreshed += 1;
    }
    state.rebuild_runtime().await?;
    Ok((refreshed, failed))
}
