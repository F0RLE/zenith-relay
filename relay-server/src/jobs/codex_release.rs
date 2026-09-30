use crate::state::AppState;
use std::sync::Arc;
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::providers::chatgpt::{
    configure_codex_client_version, refresh_codex_client_release, CODEX_RELEASE_REFRESH_INTERVAL,
};

pub(super) fn start(_state: Arc<AppState>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    // Release metadata is process-local and is not persisted in the user's
    // data directory. The state argument keeps this job's lifecycle contract
    // aligned with the other server jobs.
    tokio::spawn(async move {
        loop {
            if *shutdown.borrow() {
                break;
            }
            if let Ok(release) = refresh_codex_client_release().await {
                let _ = configure_codex_client_version(release.version());
            }
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(CODEX_RELEASE_REFRESH_INTERVAL) => {}
            }
        }
    })
}
