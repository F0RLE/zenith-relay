mod account_refresh;
mod health_probe;
pub(crate) mod quota_refresh;
mod retention;
mod wake_automation;
mod weekly_reset;

use crate::state::AppState;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

pub(crate) use account_refresh::{refresh_account_now, refresh_all_accounts_now};

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

pub(super) fn start_periodic<F, Fut>(
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
    interval_duration: Duration,
    mut run: F,
) -> JoinHandle<()>
where
    F: Send + 'static + FnMut(Arc<AppState>) -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(interval_duration);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = async {
                    interval.tick().await;
                    run(Arc::clone(&state)).await;
                } => {}
            }
        }
    })
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
