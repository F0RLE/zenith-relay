use super::super::tests::{cleanup, state};
use super::*;
use axum::{http::StatusCode, routing::get, Json, Router};
use serde_json::json;
use std::{sync::atomic::AtomicUsize, time::Duration};
use tokio::sync::{mpsc, Notify};
use zenith_relay_core::{
    protocol::RefreshStatus, scheduler::refresh::RefreshFreshness, SourceStatsStatus,
};

fn source(base_url: &str) -> ProviderSourceRecord {
    serde_json::from_value(json!({
        "id": "synthetic-source", "name": "Synthetic", "enabled": false, "inPool": false,
        "baseUrl": base_url, "secretRef": format!("source:test-{}", uuid::Uuid::new_v4()),
        "wireApi": "responses", "models": ["test"], "lastTestAt": null,
        "lastTestStatus": null, "lastError": null
    }))
    .unwrap()
}

async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (format!("http://{address}/v1"), server)
}

#[tokio::test]
async fn stats_readers_join_cache_and_project_without_a_second_provider_poll() {
    let state = state();
    let reads = Arc::new(AtomicUsize::new(0));
    let seen = reads.clone();
    let release = Arc::new(Notify::new());
    let unblock = release.clone();
    let (started, mut starts) = mpsc::unbounded_channel();
    let (base_url, server) = serve(Router::new().route(
        "/v1/usage",
        get(move || {
            let (seen, release, started) = (seen.clone(), unblock.clone(), started.clone());
            async move {
                if seen.fetch_add(1, Ordering::SeqCst) == 0 {
                    started.send(()).unwrap();
                    release.notified().await;
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
    ))
    .await;
    let source = source(&base_url);
    state
        .store()
        .unwrap()
        .upsert_source(source.clone())
        .unwrap();
    secret_store::save(&source.secret_ref, "synthetic-key").unwrap();
    let owner = state.clone();
    let first = tokio::spawn(async move { request_stats(&owner, "synthetic-source", false).await });
    tokio::time::timeout(Duration::from_secs(5), starts.recv())
        .await
        .unwrap()
        .unwrap();
    let mut second = Box::pin(request_stats(&state, &source.id, true));
    assert!(futures_util::poll!(&mut second).is_pending());
    release.notify_one();
    let good = first.await.unwrap().unwrap();
    assert_eq!(second.await.unwrap(), good);
    assert_eq!(good.balance_micro_usd, Some(12_340_000));
    let inputs = state.runtime_inputs().await.unwrap();
    assert_eq!(inputs.source_refresh[&source.id].stats, Some(good.clone()));
    assert_eq!(
        inputs.source_refresh[&source.id].state.models,
        RefreshStatus::Stale
    );
    assert_eq!(
        inputs.source_refresh[&source.id].state.balance,
        RefreshStatus::Fresh
    );
    assert_eq!(
        request_stats(&state, &source.id, false).await.unwrap(),
        good
    );
    assert_eq!(reads.load(Ordering::SeqCst), 1);

    let stale = request_stats(&state, &source.id, true).await.unwrap();
    assert_eq!(stale.as_of_ms, good.as_of_ms);
    assert_eq!(stale.balance_micro_usd, good.balance_micro_usd);
    assert_eq!(stale.refresh_error, Some(SourceStatsStatus::RateLimited));
    assert!(stale.stale);
    let stale_inputs = state.runtime_inputs().await.unwrap();
    assert_eq!(
        stale_inputs.source_refresh[&source.id].stats,
        Some(stale.clone())
    );
    assert_eq!(
        stale_inputs.source_refresh[&source.id].state.balance,
        RefreshStatus::Stale
    );
    assert_eq!(reads.load(Ordering::SeqCst), 2);

    state
        .store()
        .unwrap()
        .invalidate_source_refresh(&source.id)
        .unwrap();
    let fresh_inputs = state.runtime_inputs().await.unwrap();
    assert!(
        fresh_inputs.source_refresh[&source.id].revision
            > inputs.source_refresh[&source.id].revision
    );
    assert!(fresh_inputs.source_refresh[&source.id].stats.is_none());
    assert_eq!(
        fresh_inputs.source_refresh[&source.id].state.balance,
        RefreshStatus::Unknown
    );
    secret_store::delete(&source.secret_ref).unwrap();
    server.abort();
    cleanup(state).await;
}

#[tokio::test]
async fn missing_credentials_do_not_erase_a_same_scope_cached_observation() {
    let state = state();
    let source = source("https://provider.example.test/v1");
    state
        .store()
        .unwrap()
        .upsert_source(source.clone())
        .unwrap();
    let (_, fence) = state
        .store()
        .unwrap()
        .source_refresh_scope(&source.id)
        .unwrap();
    let stats = zenith_relay_core::SourceProviderStats::empty(
        zenith_relay_core::SourceStatsProvider::Zenith,
        SourceStatsStatus::Available,
    )
    .observed(None, 123);
    let observation = SourceStatsObservation::new(source.base_url.clone(), stats.clone());
    state
        .refresh
        .register(
            RefreshRegistration {
                identity: fence.identity(),
                kind: RefreshKind::Balance,
                origin: "https://provider.example.test".into(),
                active: false,
                automatic: false,
                due_now: false,
            },
            move |_| {
                let observation = observation.clone();
                Box::pin(async move {
                    RefreshResult {
                        value: Ok(RefreshRead::SourceStats(observation)),
                        outcome: RefreshOutcome::Success,
                    }
                })
            },
        )
        .unwrap();
    state
        .refresh
        .request(&fence.identity(), RefreshKind::Balance)
        .await
        .unwrap()
        .as_ref()
        .clone()
        .unwrap();
    // The real host callback replaces the test producer but stops before HTTP.
    assert_eq!(
        request_stats(&state, &source.id, true)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let cached = cached_stats(&state, &fence, &source.base_url).unwrap();
    assert_eq!(cached.as_of_ms, stats.as_of_ms);
    assert!(cached.stale);
    cleanup(state).await;
}

#[tokio::test]
async fn late_reads_do_not_cross_key_address_deletion_or_normalization_boundaries() {
    for kind in [RefreshKind::Models, RefreshKind::Balance] {
        for mutation in ["key", "address", "delete-readd", "normalized", "manual"] {
            let state = state();
            let release = Arc::new(Notify::new());
            let unblock = release.clone();
            let (started, mut starts) = mpsc::unbounded_channel();
            let path = if kind == RefreshKind::Models {
                "/v1/models"
            } else {
                "/v1/usage"
            };
            let (base_url, server) = serve(Router::new().route(path, get(move || {
                let (release, started) = (unblock.clone(), started.clone());
                async move {
                    started.send(()).unwrap();
                    release.notified().await;
                    Json(if kind == RefreshKind::Models {
                        json!({"data":[{"id":"observed", "supported_endpoint_types":["responses"]}]})
                    } else {
                        json!({"mode":"unrestricted", "unit":"USD", "balance":"12.34"})
                    })
                }
            }))).await;
            let mut source = source(&base_url);
            state
                .store()
                .unwrap()
                .upsert_source(source.clone())
                .unwrap();
            secret_store::save(&source.secret_ref, "synthetic-key").unwrap();
            let owner = state.clone();
            let reader = tokio::spawn(async move {
                if kind == RefreshKind::Models {
                    request_models(&owner, "synthetic-source", true)
                        .await
                        .map(|_| ())
                } else {
                    request_stats(&owner, "synthetic-source", true)
                        .await
                        .map(|_| ())
                }
            });
            tokio::time::timeout(Duration::from_secs(5), starts.recv())
                .await
                .unwrap()
                .unwrap();
            {
                let _mutation = state.setup_guard().await;
                let mut store = state.store().unwrap();
                match mutation {
                    "key" => {
                        store.invalidate_source_refresh(&source.id).unwrap();
                        secret_store::save(&source.secret_ref, "synthetic-replacement").unwrap();
                    }
                    "delete-readd" => {
                        store.replace_records(vec![], vec![]).unwrap();
                        store.upsert_source(source.clone()).unwrap();
                    }
                    "normalized" => {
                        let (_, fence) = store.source_refresh_scope(&source.id).unwrap();
                        store
                            .apply_source_refresh(&fence, |source| {
                                source.base_url.push_str("/normalized");
                                Ok(())
                            })
                            .unwrap();
                    }
                    "manual" => {
                        source.last_test_status = Some("manual".into());
                        store.upsert_source(source.clone()).unwrap();
                    }
                    _ => {
                        source.base_url.push_str("/changed");
                        store.upsert_source(source.clone()).unwrap();
                    }
                }
            }
            release.notify_one();
            let error = tokio::time::timeout(Duration::from_secs(5), reader)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::Conflict, "{kind:?}: {mutation}");
            let saved = state.store().unwrap().source(&source.id).unwrap().clone();
            assert_eq!(saved.models, ["test"]);
            assert!(
                state.runtime_inputs().await.unwrap().source_refresh[&source.id]
                    .stats
                    .is_none()
            );
            secret_store::delete(&source.secret_ref).unwrap();
            server.abort();
            cleanup(state).await;
        }
    }
}

#[tokio::test]
async fn manual_catalog_is_checked_after_waiting_for_the_configuration_owner() {
    let state = state();
    let mut source = source("https://provider.example.test/v1");
    state
        .store()
        .unwrap()
        .upsert_source(source.clone())
        .unwrap();
    let guard = state.setup_guard().await;
    let mut reader = Box::pin(request_models(&state, &source.id, false));
    assert!(futures_util::poll!(&mut reader).is_pending());
    source.last_test_status = Some("manual".into());
    state
        .store()
        .unwrap()
        .upsert_source(source.clone())
        .unwrap();
    drop(guard);
    assert_eq!(reader.await.unwrap(), source);
    let (_, fence) = state
        .store()
        .unwrap()
        .source_refresh_scope(&source.id)
        .unwrap();
    assert_eq!(
        state
            .refresh
            .freshness(&fence.identity(), RefreshKind::Models),
        RefreshFreshness::Unknown
    );
    cleanup(state).await;
}
