mod health_probe;
pub(crate) mod quota_refresh;
mod retention;
mod wake_automation;

use crate::state::{AppState, ServerAccountRecord};
use futures_util::{stream, StreamExt};
use std::sync::Arc;
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::accounts::automatic_quota_refresh_eligible;

const QUOTA_REFRESH_BATCH_SIZE: usize = 5;

pub struct BackgroundJobs {
    handles: Vec<JoinHandle<()>>,
}

impl BackgroundJobs {
    pub async fn join(self) -> Result<(), String> {
        for handle in self.handles {
            handle
                .await
                .map_err(|error| format!("background job failed: {error}"))?;
        }
        Ok(())
    }
}

pub fn start(state: Arc<AppState>, shutdown: watch::Receiver<bool>) -> BackgroundJobs {
    BackgroundJobs {
        handles: vec![
            health_probe::start(state.clone(), shutdown.clone()),
            quota_refresh::start(state.clone(), shutdown.clone()),
            retention::start(state.clone(), shutdown.clone()),
            wake_automation::start(state, shutdown),
        ],
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        store::{Store, Vault},
    };
    use tempfile::TempDir;

    #[tokio::test]
    async fn background_jobs_stop_on_shutdown_signal() {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        let state = AppState::new(config, store, vault).unwrap();
        let (shutdown, receiver) = watch::channel(false);
        let jobs = start(state, receiver);

        shutdown.send(true).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), jobs.join())
            .await
            .unwrap()
            .unwrap();
    }
}
