use crate::state::AppState;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};
use zenith_relay_core::{discover_source_with_protocol_config, ProviderSource};

const INTERVAL: Duration = Duration::from_secs(8 * 60 * 60);

pub fn start(state: Arc<AppState>, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
    super::start_periodic(state, shutdown, INTERVAL, |state| async move {
        let _ = run(&state).await;
    })
}

async fn run(state: &Arc<AppState>) -> Result<(), String> {
    let records = state.store.sources()?;
    for checked in records {
        let record = &checked;
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
            api_key: api_key.clone(),
            wire_api: record.wire_api,
            models: record.models.clone(),
        };
        let discovery = discover_source_with_protocol_config(
            &source,
            &record.protocol_bindings,
            &record.protocol_config,
        )
        .await;
        let _configuration = state.configuration_lock.lock().await;
        let Some(mut record) = state
            .store
            .sources()?
            .into_iter()
            .find(|record| record.id == checked.id)
        else {
            continue;
        };
        if record.protocol_config != checked.protocol_config
            || record.base_url != checked.base_url
            || record.wire_api != checked.wire_api
            || record.protocol_bindings != checked.protocol_bindings
            || record.models != checked.models
            || state.vault.load(&record.secret_ref)?.as_deref() != Some(api_key.as_str())
        {
            continue;
        }
        let previous = record.clone();
        match discovery {
            Ok(discovery) => {
                let mut protocol_config = record.protocol_config.clone();
                protocol_config.merge_catalog(discovery.capabilities);
                let resolved_base_url = discovery.resolved_base_url.clone();
                if resolved_base_url
                    .as_deref()
                    .is_some_and(|base_url| record.base_url != base_url)
                    || record.models != discovery.models
                    || record.protocol_bindings != discovery.protocol_bindings
                    || record.detected_model_prices != discovery.detected_model_prices
                    || record.protocol_config != protocol_config
                    || record.last_error_code.is_some()
                {
                    if let Some(base_url) = resolved_base_url {
                        record.base_url = base_url;
                    }
                    record.models = discovery.models;
                    record.protocol_bindings = discovery.protocol_bindings;
                    record.detected_model_prices = discovery.detected_model_prices;
                    record.protocol_config = protocol_config;
                    record.last_error_code = None;
                    state.store.save_source(&record)?;
                    state
                        .rebuild_runtime_or_rollback(|| state.store.save_source(&previous))
                        .await?;
                }
            }
            Err(_) => {
                if record.last_error_code.as_deref() != Some("health_probe_failed") {
                    record.last_error_code = Some("health_probe_failed".to_string());
                    state.store.save_source(&record)?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        config::Config,
        state::SourceRecord,
        store::{Store, Vault},
    };
    use axum::{routing::get, Json, Router};
    use serde_json::json;
    use tempfile::TempDir;
    use zenith_relay_core::WireApi;

    #[tokio::test]
    async fn background_discovery_preserves_concurrent_edits_and_updates_endpoints_only() {
        for mutation in ["none", "policy", "key", "address", "delete"] {
            let root = TempDir::new().unwrap();
            let config = Config::for_test(root.path().into(), "127.0.0.1:0".parse().unwrap());
            let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
            let vault =
                Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
            let state = AppState::new(config, store, vault).unwrap();
            let (started, mut requests) = tokio::sync::mpsc::unbounded_channel();
            let release = Arc::new(tokio::sync::Notify::new());
            let handler_release = release.clone();
            let app = Router::new().route("/v1/models", get(move || {
                let started = started.clone();
                let release = handler_release.clone();
                async move {
                    started.send(()).unwrap();
                    release.notified().await;
                    Json(json!({"data":[{"id":"test", "supported_endpoint_types":["messages"], "supported_reasoning_efforts":["low","high"]}]}))
                }
            }));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let mut record: SourceRecord = serde_json::from_value(json!({
                "id":"source", "name":"Synthetic", "enabled":true, "inPool":true,
                "draining":false, "baseUrl":format!("http://{address}/v1"),
                "secretRef":"source:test", "wireApi":"responses", "models":["test"],
                "allowedModels":[], "excludedModels":[], "priority":0, "weight":1
            }))
            .unwrap();
            state.store.save_source(&record).unwrap();
            state
                .vault
                .save(&record.secret_ref, "synthetic-test-key")
                .unwrap();
            let worker_state = state.clone();
            let worker = tokio::spawn(async move { run(&worker_state).await });
            tokio::time::timeout(Duration::from_secs(5), requests.recv())
                .await
                .unwrap()
                .unwrap();
            {
                let _configuration = state.configuration_lock.lock().await;
                match mutation {
                    "policy" => {
                        record.priority = 7;
                        state.store.save_source(&record).unwrap();
                    }
                    "key" => state
                        .vault
                        .save(&record.secret_ref, "synthetic-new-key")
                        .unwrap(),
                    "address" => {
                        record.base_url = "https://changed.example.test/v1".into();
                        state.store.save_source(&record).unwrap();
                    }
                    "delete" => {
                        state.store.delete_source(&record.id).unwrap();
                    }
                    _ => {}
                }
            }
            release.notify_one();
            tokio::time::timeout(Duration::from_secs(10), worker)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            let saved = state.store.sources().unwrap();
            if mutation == "delete" {
                assert!(saved.is_empty());
            } else {
                let saved = &saved[0];
                assert_eq!(saved.priority, record.priority);
                assert_eq!(saved.base_url, record.base_url);
                if matches!(mutation, "none" | "policy") {
                    assert!(saved.protocol_config.capabilities.iter().any(|entry| {
                        entry.upstream_wire_api == WireApi::Messages
                            && entry.status.available()
                            && entry.reasoning_efforts.is_empty()
                    }));
                    assert!(saved
                        .effective_protocol_bindings()
                        .unwrap()
                        .iter()
                        .all(|binding| {
                            binding
                                .adapter
                                .upstream_protocol(binding.wire_api)
                                .wire_api()
                                == WireApi::Messages
                        }));
                } else {
                    assert!(saved.protocol_config.capabilities.is_empty());
                }
            }
            state.shutdown_runtime().await.unwrap();
            server.abort();
        }
    }
}
