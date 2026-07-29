use crate::state::{now_ms, AppState};
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const IMPORT_TTL_MS: u64 = 30 * 60 * 1_000;

pub fn start(state: Arc<AppState>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(INTERVAL);
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
                    let _ = run(&state).await;
                } => {}
            }
        }
    })
}

async fn run(state: &AppState) -> Result<(), String> {
    let now_ms = now_ms();
    state.store.prune_usage_history(now_ms)?;
    for secret_ref in state
        .store
        .delete_pending_imports_before(now_ms.saturating_sub(IMPORT_TTL_MS))?
    {
        let _ = state.vault.delete(&secret_ref);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        store::{PendingImport, Store, Vault},
    };
    use tempfile::TempDir;

    #[tokio::test]
    async fn retention_removes_expired_import_payload_and_secret() {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        vault.save("import:expired", "synthetic-secret").unwrap();
        store
            .save_pending_import(&PendingImport {
                id: "expired".into(),
                preview_json: "{}".into(),
                secret_ref: "import:expired".into(),
                created_at_ms: 1,
            })
            .unwrap();
        let state = AppState::new(config, store.clone(), vault.clone()).unwrap();

        run(&state).await.unwrap();

        assert!(store.pending_import("expired").unwrap().is_none());
        assert!(vault.load("import:expired").unwrap().is_none());
    }
}
