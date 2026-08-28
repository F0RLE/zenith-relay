use crate::state::AppState;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::{discover_source_models_and_protocol_bindings, ProviderSource};

const INTERVAL: Duration = Duration::from_secs(8 * 60 * 60);

pub fn start(state: Arc<AppState>, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    super::start_periodic(state, shutdown, INTERVAL, |state| async move {
        let _ = run(&state).await;
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
        match discover_source_models_and_protocol_bindings(&source, &record.protocol_bindings).await
        {
            Ok(discovery) if !discovery.models.is_empty() => {
                let resolved_base_url = discovery.resolved_base_url.clone();
                if resolved_base_url
                    .as_deref()
                    .is_some_and(|base_url| record.base_url != base_url)
                    || record.models != discovery.models
                    || record.protocol_bindings != discovery.protocol_bindings
                    || record.last_error_code.is_some()
                {
                    if let Some(base_url) = resolved_base_url {
                        record.base_url = base_url;
                    }
                    record.models = discovery.models;
                    record.protocol_bindings = discovery.protocol_bindings;
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
