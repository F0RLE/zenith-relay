mod account_refresh;
mod codex_release;
mod health_probe;
mod model_metadata;
mod pricing;
pub(crate) mod quota_refresh;
mod retention;
mod wake_automation;
mod weekly_reset;

use crate::state::AppState;
use std::{future::Future, sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::{
    model_metadata::ModelMetadataCatalogLoader,
    pricing::{pricing_refresh_delay, CatalogRefreshDeadline, PricingCatalogLoader},
};

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

trait ScheduledCatalog: Clone + Send + Sync + 'static {
    fn next_deadline(&self, now_ms: u64) -> CatalogRefreshDeadline;
    fn refresh_due(&self, now_ms: u64) -> bool;
    fn wait_for_change(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>>;
    fn refresh(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

macro_rules! scheduled_catalog_impl {
    ($loader:ty) => {
        impl ScheduledCatalog for $loader {
            fn next_deadline(&self, now_ms: u64) -> CatalogRefreshDeadline {
                self.next_refresh_deadline(now_ms)
            }

            fn refresh_due(&self, now_ms: u64) -> bool {
                self.refresh_due(now_ms)
            }

            fn wait_for_change(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                Box::pin(self.wait_for_schedule_change())
            }

            fn refresh(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
                Box::pin(async {
                    let _ = self.refresh(false).await;
                })
            }
        }
    };
}

scheduled_catalog_impl!(PricingCatalogLoader);
scheduled_catalog_impl!(ModelMetadataCatalogLoader);

impl<S> ScheduledCatalog for Arc<S>
where
    S: ScheduledCatalog,
{
    fn next_deadline(&self, now_ms: u64) -> CatalogRefreshDeadline {
        self.as_ref().next_deadline(now_ms)
    }

    fn refresh_due(&self, now_ms: u64) -> bool {
        self.as_ref().refresh_due(now_ms)
    }

    fn wait_for_change(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.as_ref().wait_for_change()
    }

    fn refresh(&self) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        self.as_ref().refresh()
    }
}

fn start_catalog_job<S, L>(
    state: Arc<AppState>,
    mut shutdown: watch::Receiver<bool>,
    load: L,
) -> JoinHandle<()>
where
    S: ScheduledCatalog,
    L: Fn(&AppState) -> S + Send + Sync + 'static,
{
    let instance_id = state.capabilities.server_id.clone();
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            let loader = load(&state);
            let now_ms = zenith_relay_core::unix_time_ms();
            let delay = pricing_refresh_delay(&instance_id, loader.next_deadline(now_ms), now_ms);
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
                _ = tokio::time::sleep(delay) => {}
                _ = loader.wait_for_change() => continue,
            }
            if *shutdown.borrow() {
                break;
            }
            if loader.refresh_due(zenith_relay_core::unix_time_ms()) {
                loader.refresh().await;
            }
        }
    })
}

pub fn start(state: Arc<AppState>, shutdown: watch::Receiver<bool>) -> BackgroundJobs {
    BackgroundJobs {
        handles: vec![
            codex_release::start(state.clone(), shutdown.clone()),
            health_probe::start(state.clone(), shutdown.clone()),
            model_metadata::start(state.clone(), shutdown.clone()),
            pricing::start(state.clone(), shutdown.clone()),
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
