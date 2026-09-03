use crate::state::AppState;
use std::sync::Arc;
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::pricing::pricing_refresh_delay;

pub(super) fn start(state: Arc<AppState>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    let instance_id = state.capabilities.server_id.clone();
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            let loader = state.pricing_loader();
            let now_ms = zenith_relay_core::unix_time_ms();
            let deadline = loader.next_refresh_deadline(now_ms);
            let delay = pricing_refresh_delay(&instance_id, deadline, now_ms);
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                    continue;
                }
                _ = tokio::time::sleep(delay) => {}
                _ = loader.wait_for_schedule_change() => continue,
            }
            if *shutdown.borrow() {
                break;
            }
            let loader = state.pricing_loader();
            if loader.refresh_due(zenith_relay_core::unix_time_ms()) {
                let _ = loader.refresh(false).await;
            }
        }
    })
}
