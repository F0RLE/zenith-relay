use super::*;
use crate::config::Config;
use crate::store::{Store, Vault};
use axum::extract::{Path, State};
use axum::routing::post;
use std::collections::BTreeMap;
use tempfile::TempDir;
use zenith_relay_core::Error;

fn test_state(root: &TempDir, port: u16) -> Arc<AppState> {
    let config = Config::for_test(
        root.path().to_path_buf(),
        format!("127.0.0.1:{port}").parse().unwrap(),
    );
    let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
    AppState::new(config, store, vault).unwrap()
}

use crate::test_fixtures::pooled_source;

#[tokio::test]
async fn source_probe_rejects_delete_and_readd_with_identical_configuration() {
    let root = TempDir::new().unwrap();
    let state = test_state(&root, 0);
    let (started, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let handler_release = release.clone();
    let app = Router::new().route(
        "/v1/responses",
        post(move || {
            let (started, release) = (started.clone(), handler_release.clone());
            async move {
                started.send(()).unwrap();
                release.notified().await;
                Json(serde_json::json!({
                    "status": "completed",
                    "output": [{"type": "message", "content": [{
                        "type": "output_text", "text": "OK"
                    }]}]
                }))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut source = pooled_source("source-readded", "test-model");
    source.base_url = format!("http://{address}/v1");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-key")
        .unwrap();

    let worker_state = state.clone();
    let source_id = source.id.clone();
    let probe = tokio::spawn(async move {
        probe_source(
            State(worker_state),
            Path(source_id),
            Json(zenith_relay_core::SourceProbeInput {
                model_id: "test-model".into(),
                wire_api: WireApi::Responses,
                expected_revision: 0,
            }),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    {
        let _configuration = state.configuration_lock.lock().await;
        state.store.delete_source(&source.id).unwrap();
        state.store.save_source(&source).unwrap();
    }
    release.notify_one();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), probe)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err().status, StatusCode::CONFLICT);
    assert!(state.store.sources().unwrap()[0]
        .protocol_config
        .capabilities
        .is_empty());
    state.shutdown_runtime().await.unwrap();
    state.refresh.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn source_policy_patch_updates_all_routes_without_replacing_the_runtime() {
    let root = TempDir::new().unwrap();
    let state = test_state(&root, 0);
    let source = pooled_source("synthetic-source", "model-a");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let runtime = state.runtime().unwrap().unwrap();

    let Json(summary) = update_source(
        State(state.clone()),
        Path(source.id.clone()),
        Json(SourcePatch {
            enabled: Some(false),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    assert!(!summary.enabled);
    assert!(!state.store.sources().unwrap()[0].enabled);
    assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));
    assert!(runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn source_endpoint_patch_retires_the_previous_transport() {
    let root = TempDir::new().unwrap();
    let state = test_state(&root, 0);
    let source = pooled_source("endpoint-source", "model-a");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let previous = state.runtime().unwrap().unwrap();

    let Json(summary) = update_source(
        State(state.clone()),
        Path(source.id.clone()),
        Json(SourcePatch {
            base_url: Some("https://changed.example.test/v1".into()),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    assert_eq!(summary.base_url, "https://changed.example.test/v1");
    let current = state.runtime().unwrap().unwrap();
    assert!(!Arc::ptr_eq(&previous, &current));
    assert!(previous
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    assert!(current
        .candidate_runtime_order()
        .iter()
        .any(|candidate| candidate.available));
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn source_delete_retires_all_routes_of_the_old_runtime() {
    let root = TempDir::new().unwrap();
    let state = test_state(&root, 0);
    let source = pooled_source("removed-source", "model-a");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let old_runtime = state.runtime().unwrap().unwrap();

    delete_source(State(state.clone()), Path(source.id.clone()))
        .await
        .unwrap();
    assert!(state.store.sources().unwrap().is_empty());
    assert!(state.runtime().unwrap().is_none());
    assert!(old_runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
}

#[test]
fn upstream_404_is_a_non_retryable_bad_gateway_error() {
    let error = source_discovery_error(Error::UpstreamStatus(404));

    assert_eq!(error.status, StatusCode::BAD_GATEWAY);
    assert_eq!(error.code, "source_test_failed");
    assert_eq!(error.stage, "upstream");
    assert!(!error.retryable);
    assert!(error.message.contains("404"));
}

#[test]
fn upstream_server_failures_remain_retryable() {
    let error = source_discovery_error(Error::UpstreamStatus(503));

    assert_eq!(error.status, StatusCode::BAD_GATEWAY);
    assert!(error.retryable);
}

#[test]
fn failed_refresh_clears_the_source_catalog() {
    let mut record = SourceRecord {
        id: "source".to_string(),
        name: "Provider".to_string(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "https://provider.test/v1".to_string(),
        secret_ref: "source:source".to_string(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: zenith_relay_core::SourceAdapter::Native,
            reasoning_mode: zenith_relay_core::MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["model-a".to_string()],
        }],
        models: vec!["model-a".to_string()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::from([(
            "model-a".to_string(),
            ApiModelPriceOverride {
                input_micro_usd_per_million: 1,
                cached_input_micro_usd_per_million: None,
                cache_write_5m_micro_usd_per_million: None,
                cache_write_1h_micro_usd_per_million: None,
                output_micro_usd_per_million: 1,
            },
        )]),
        last_error_code: None,
    };

    clear_source_catalog(&mut record);

    assert!(record.models.is_empty());
    assert!(record.detected_model_prices.is_empty());
    assert!(record.protocol_bindings[0].model_ids.is_empty());
}

#[tokio::test]
async fn source_stats_rejects_a_self_route_before_contacting_gateway() {
    let root = TempDir::new().unwrap();
    let state = test_state(&root, 45_678);
    let record = SourceRecord {
        id: "self-source".to_string(),
        name: "Self source".to_string(),
        enabled: true,
        in_pool: false,
        draining: false,
        base_url: format!(
            "{}/v1",
            state.config.public_base_url.as_str().trim_end_matches('/')
        ),
        secret_ref: "source:self-source".to_string(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["provider/model".to_string()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        last_error_code: None,
    };
    state.store.save_source(&record).unwrap();
    state
        .vault
        .save(&record.secret_ref, "source-secret")
        .unwrap();

    let error = source_stats(
        State(state),
        Path(record.id),
        Query(SourceStatsQuery::default()),
    )
    .await
    .expect_err("a source pointing at this Relay must be rejected");

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.code, "source_self_route");
    assert_eq!(error.stage, "validation");
    assert!(!error.retryable);
}
