use crate::state::AppState;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::{discover_source_models, ProviderSource};

const INTERVAL: Duration = Duration::from_secs(300);

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

async fn run(state: &Arc<AppState>) -> Result<(), String> {
    let records = state.store.sources()?;
    let mut changed = false;
    for mut record in records {
        if !record.enabled || record.draining {
            continue;
        }
        let Some(api_key) = state.vault.load(&record.secret_ref)? else {
            continue;
        };
        let source = ProviderSource {
            id: record.id.clone(),
            name: record.name.clone(),
            base_url: record.base_url.clone(),
            api_key,
            wire_api: record.wire_api,
            models: record.models.clone(),
        };
        match discover_source_models(&source).await {
            Ok(models) if !models.is_empty() => {
                if record.models != models || record.last_error_code.is_some() {
                    record.models = models;
                    record.last_error_code = None;
                    state.store.save_source(&record)?;
                    changed = true;
                }
            }
            Ok(_) => {}
            Err(_) => {
                if record.last_error_code.as_deref() != Some("health_probe_failed") {
                    record.last_error_code = Some("health_probe_failed".to_string());
                    state.store.save_source(&record)?;
                    changed = true;
                }
            }
        }
    }
    if changed {
        state.rebuild_runtime().await?;
    }
    Ok(())
}
