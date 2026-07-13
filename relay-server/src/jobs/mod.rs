mod health_probe;
pub(crate) mod quota_refresh;
mod wake_automation;

use crate::state::{AppState, ServerAccountRecord};
use futures_util::{stream, StreamExt};
use std::sync::Arc;

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
        quota_refresh::refresh_account_metadata(state, account, false).await?;
    if !transitions.is_empty() {
        wake_automation::schedule_transitions(state, &updated, &transitions).await?;
    }
    let _ = state.rebuild_runtime().await;
    Ok(updated)
}

pub(crate) async fn refresh_pool_accounts_now(
    state: &Arc<AppState>,
) -> Result<(usize, usize), String> {
    let accounts = state
        .store
        .accounts()?
        .into_iter()
        .filter(|account| account.in_pool && account.enabled && !account.draining)
        .collect::<Vec<_>>();
    let results = stream::iter(accounts.into_iter().map(|account| {
        let state = Arc::clone(state);
        async move { quota_refresh::refresh_account_metadata(&state, account, false).await }
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
