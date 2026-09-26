use super::*;
use crate::{
    config::Config,
    store::{Store, Vault},
};
use axum::{routing::get, Json, Router};
use serde_json::json;
use tempfile::TempDir;
use zenith_relay_core::{protocol::RefreshStatus, SourceStatsStatus};

#[tokio::test]
async fn source_catalog_commit_waits_for_runtime_build_before_changing_routes() {
    let root = TempDir::new().unwrap();
    let config = Config::for_test(root.path().into(), "127.0.0.1:0".parse().unwrap());
    let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
    let state = AppState::new(config, store, vault).unwrap();
    let (started, mut requests) = tokio::sync::mpsc::unbounded_channel();
    let release = Arc::new(tokio::sync::Notify::new());
    let handler_release = release.clone();
    let app = Router::new().route(
        "/v1/models",
        get(move || {
            let (started, release) = (started.clone(), handler_release.clone());
            async move {
                started.send(()).unwrap();
                release.notified().await;
                Json(json!({"data":[{"id":"updated", "supported_endpoint_types":["responses"]}]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut record = crate::test_fixtures::pooled_source("source", "test");
    record.base_url = format!("http://{address}/v1");
    state.store.save_source(&record).unwrap();
    state
        .vault
        .save(&record.secret_ref, "synthetic-key")
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let previous_runtime = state.runtime().unwrap().unwrap();

    let worker_state = state.clone();
    let worker = tokio::spawn(async move { request_models(&worker_state, "source").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
        .await
        .unwrap()
        .unwrap();
    let build = state.lock_runtime_rebuild().await;
    release.notify_one();
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert_eq!(
        state.store.sources().unwrap()[0].models,
        ["test"],
        "catalog routes must not commit while the old runtime can still dispatch"
    );
    assert!(previous_runtime
        .candidate_runtime_order()
        .iter()
        .any(|candidate| candidate.available));
    drop(build);

    let updated = tokio::time::timeout(std::time::Duration::from_secs(10), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(updated.models, ["updated"]);
    assert!(previous_runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    state.shutdown_runtime().await.unwrap();
    server.abort();
    state.refresh.shutdown().await;
}

#[tokio::test]
async fn late_models_cannot_overwrite_a_changed_source() {
    for mutation in ["none", "policy", "key", "address", "delete"] {
        let root = TempDir::new().unwrap();
        let config = Config::for_test(root.path().into(), "127.0.0.1:0".parse().unwrap());
        let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
        let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
        let state = AppState::new(config, store, vault).unwrap();
        let (started, mut requests) = tokio::sync::mpsc::unbounded_channel();
        let release = Arc::new(tokio::sync::Notify::new());
        let handler_release = release.clone();
        let app = Router::new().route(
            "/v1/models",
            get(move || {
                let (started, release) = (started.clone(), handler_release.clone());
                async move {
                    started.send(()).unwrap();
                    release.notified().await;
                    Json(json!({"data":[{"id":"test", "supported_endpoint_types":["messages"]}]}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut record: SourceRecord = serde_json::from_value(json!({
            "id":"source", "name":"Synthetic", "enabled":false, "inPool":false,
            "draining":false, "baseUrl":format!("http://{address}/v1"),
            "secretRef":"source:synthetic", "wireApi":"responses", "models":["test"],
            "allowedModels":[], "excludedModels":[], "priority":0, "weight":1
        }))
        .unwrap();
        state.store.save_source(&record).unwrap();
        state
            .vault
            .save(&record.secret_ref, "synthetic-key")
            .unwrap();
        let worker_state = state.clone();
        let worker = tokio::spawn(async move { request_models(&worker_state, "source").await });
        tokio::time::timeout(std::time::Duration::from_secs(5), requests.recv())
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
                "key" => {
                    state.store.invalidate_source_refresh(&record.id).unwrap();
                    state
                        .vault
                        .save(&record.secret_ref, "new-synthetic-key")
                        .unwrap();
                }
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
        let result = tokio::time::timeout(std::time::Duration::from_secs(10), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result.is_ok(), matches!(mutation, "none" | "policy"));
        let saved = state.store.sources().unwrap();
        if mutation == "delete" {
            assert!(saved.is_empty());
        } else if matches!(mutation, "none" | "policy") {
            assert!(saved[0]
                .protocol_config
                .capabilities
                .iter()
                .any(|cap| cap.status.available()));
        } else {
            assert!(saved[0].protocol_config.capabilities.is_empty());
        }
        state.shutdown_runtime().await.unwrap();
        server.abort();
        state.refresh.shutdown().await;
    }
}

#[tokio::test]
async fn balance_reads_cache_last_good_until_forced_and_retain_a_stale_warning() {
    use axum::http::StatusCode;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let root = TempDir::new().unwrap();
    let config = Config::for_test(root.path().into(), "127.0.0.1:0".parse().unwrap());
    let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
    let state = AppState::new(config, store, vault).unwrap();
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = reads.clone();
    let app = Router::new().route(
        "/v1/usage",
        get(move || {
            let seen = seen.clone();
            async move {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    (
                        StatusCode::OK,
                        Json(json!({"mode":"unrestricted", "unit":"USD", "balance":"12.34"})),
                    )
                } else {
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        Json(json!({"error":"rate_limited"})),
                    )
                }
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let record: SourceRecord = serde_json::from_value(json!({
        "id":"source", "name":"Synthetic", "enabled":false, "inPool":false,
        "draining":false, "baseUrl":format!("http://{address}/v1"),
        "secretRef":"source:synthetic", "wireApi":"responses", "models":["test"],
        "allowedModels":[], "excludedModels":[], "priority":0, "weight":1
    }))
    .unwrap();
    state.store.save_source(&record).unwrap();
    state
        .vault
        .save(&record.secret_ref, "synthetic-key")
        .unwrap();
    let before_revision = state.snapshot().unwrap().sources[0]
        .refresh_revision
        .unwrap();
    assert_eq!(
        state.snapshot().unwrap().sources[0].refresh_state.models,
        RefreshStatus::Stale
    );
    assert_eq!(
        state.snapshot().unwrap().sources[0].refresh_state.balance,
        RefreshStatus::Unknown
    );
    let good = request_stats(&state, "source", false).await.unwrap();
    assert_eq!(good.balance_micro_usd, Some(12_340_000));
    assert!(good.as_of_ms.is_some());
    assert_eq!(
        state.snapshot().unwrap().sources[0].provider_stats,
        Some(good.clone())
    );
    assert_eq!(
        state.snapshot().unwrap().sources[0].refresh_state.balance,
        RefreshStatus::Fresh
    );
    assert_eq!(
        request_stats(&state, "source", false)
            .await
            .unwrap()
            .as_of_ms,
        good.as_of_ms
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    let stale = request_stats(&state, "source", true).await.unwrap();
    assert_eq!(stale.balance_micro_usd, good.balance_micro_usd);
    assert!(stale.stale);
    assert_eq!(stale.refresh_error, Some(SourceStatsStatus::RateLimited));
    assert_eq!(
        state.snapshot().unwrap().sources[0].provider_stats,
        Some(stale.clone())
    );
    assert_eq!(
        state.snapshot().unwrap().sources[0].refresh_state.balance,
        RefreshStatus::Stale
    );
    assert_eq!(
        request_stats(&state, "source", false)
            .await
            .unwrap()
            .refresh_error,
        stale.refresh_error
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);
    state.store.invalidate_source_refresh("source").unwrap();
    let after_revision = state.snapshot().unwrap().sources[0]
        .refresh_revision
        .unwrap();
    assert!(after_revision > before_revision);
    assert!(state.snapshot().unwrap().sources[0]
        .provider_stats
        .is_none());
    assert_eq!(
        state.snapshot().unwrap().sources[0].refresh_state.balance,
        RefreshStatus::Unknown
    );
    let after_replacement = request_stats(&state, "source", false).await.unwrap();
    assert_eq!(after_replacement.status, SourceStatsStatus::RateLimited);
    assert!(after_replacement.balance_micro_usd.is_none());
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    state.refresh.shutdown().await;
    server.abort();
}
