use super::super::tests::{cleanup, state};
use super::*;
use crate::local_pool::store::secret_store;
use axum::{Json, Router, http::StatusCode, routing::get};
use serde_json::json;
use std::{sync::atomic::AtomicUsize, time::Duration};
use tokio::sync::{Notify, mpsc};
use zenith_relay_core::{
    CapabilityOrigin, CapabilityStatus, ProtocolFeature, SourceStatsStatus, WireApi,
    protocol::RefreshStatus, scheduler::refresh::RefreshFreshness,
    scheduler::refresh::SourceStatsObservation,
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

fn source_with_catalog(base_url: &str) -> ProviderSourceRecord {
    let mut source = source(base_url);
    source.protocol_config = serde_json::from_value(json!({
        "capabilities": [
            {"modelId": "test", "upstreamWireApi": "responses", "status": "declared",
                "origin": "catalog", "checkedAtMs": 2},
            {"modelId": "test", "upstreamWireApi": "responses", "status": "unknown",
                "origin": "manual", "checkedAtMs": 1,
                "features": {"reasoning": "unsupported"}}
        ]
    }))
    .unwrap();
    source.protocol_bindings = source
        .protocol_config
        .resolve(&source.base_url, &source.models, &[], source.wire_api)
        .unwrap();
    source.detected_model_prices.insert(
        "test".into(),
        zenith_relay_core::ApiModelPriceOverride {
            input_micro_usd_per_million: 11,
            output_micro_usd_per_million: 22,
            ..Default::default()
        },
    );
    source
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
    let inputs = state.snapshot_inputs().await.unwrap();
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

    let stale_stats = request_stats(&state, &source.id, true).await.unwrap();
    assert_eq!(stale_stats.as_of_ms, good.as_of_ms);
    assert_eq!(stale_stats.balance_micro_usd, good.balance_micro_usd);
    assert_eq!(
        stale_stats.refresh_error,
        Some(SourceStatsStatus::RateLimited)
    );
    assert!(stale_stats.stale);
    let stale_inputs = state.snapshot_inputs().await.unwrap();
    assert_eq!(
        stale_inputs.source_refresh[&source.id].stats,
        Some(stale_stats.clone())
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
    let fresh_inputs = state.snapshot_inputs().await.unwrap();
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
    let empty_stats = zenith_relay_core::SourceProviderStats::empty(
        zenith_relay_core::SourceStatsProvider::Zenith,
        SourceStatsStatus::Available,
    )
    .observed(None, 123);
    let observation = SourceStatsObservation::new(source.base_url.clone(), empty_stats.clone());
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
                        refresh_value: Ok(RefreshRead::SourceStats(observation)),
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
    assert_eq!(cached.as_of_ms, empty_stats.as_of_ms);
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
                state.snapshot_inputs().await.unwrap().source_refresh[&source.id]
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
async fn partial_catalog_refresh_preserves_failed_history_and_recovers_on_a_valid_empty_read() {
    let state = state();
    let visible = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ready = visible.clone();
    let (base_url, server) = serve(Router::new().route(
        "/v1/models",
        get(move |headers: axum::http::HeaderMap| {
            let ready = ready.clone();
            async move {
                if headers.contains_key("x-api-key") {
                    if ready.load(Ordering::SeqCst) {
                        (StatusCode::OK, Json(json!({"data": []})))
                    } else {
                        (
                            StatusCode::FORBIDDEN,
                            Json(json!({"error": "synthetic failure"})),
                        )
                    }
                } else {
                    (
                        StatusCode::OK,
                        Json(json!({"data": [{"id": "new-responses",
                        "supported_endpoint_types": ["responses"],
                        "inputMicroUsdPerMillion": 33, "outputMicroUsdPerMillion": 44}]})),
                    )
                }
            }
        }),
    ))
    .await;
    let mut source = source(&base_url);
    source.models = vec!["old-responses".into(), "old-messages".into()];
    source.protocol_bindings = vec![
        zenith_relay_core::SourceProtocolBinding::legacy(
            WireApi::Responses,
            &["old-responses".into()],
        ),
        zenith_relay_core::SourceProtocolBinding::legacy(
            WireApi::Messages,
            &["old-messages".into()],
        ),
    ];
    source.protocol_config = serde_json::from_value(json!({"capabilities": [
        {"modelId": "old-messages", "upstreamWireApi": "messages", "status": "declared",
            "origin": "catalog", "checkedAtMs": 1, "features": {"reasoning": "unsupported"}},
        {"modelId": "old-messages", "upstreamWireApi": "responses", "status": "unsupported",
            "origin": "catalog", "checkedAtMs": 1}
    ]}))
    .unwrap();
    source.detected_model_prices.insert(
        "old-messages".into(),
        zenith_relay_core::ApiModelPriceOverride {
            input_micro_usd_per_million: 11,
            output_micro_usd_per_million: 22,
            ..Default::default()
        },
    );
    state
        .store()
        .unwrap()
        .upsert_source(source.clone())
        .unwrap();
    secret_store::save(&source.secret_ref, "synthetic-key").unwrap();
    let partial = request_models(&state, &source.id, true).await.unwrap();
    assert_eq!(partial.models, ["new-responses", "old-messages"]);
    assert_eq!(partial.protocol_bindings.len(), 2);
    assert_eq!(partial.last_test_status.as_deref(), Some("error"));
    assert!(partial.last_error.as_ref().unwrap().contains("403"));
    assert_eq!(
        partial.detected_model_prices["old-messages"].input_micro_usd_per_million,
        11
    );
    assert_eq!(
        partial.detected_model_prices["new-responses"].input_micro_usd_per_million,
        33
    );
    assert_eq!(partial.enabled, source.enabled);
    assert_eq!(partial.in_pool, source.in_pool);
    let routes = partial.effective_protocol_bindings().unwrap();
    let old = routes
        .iter()
        .filter(|route| route.model_ids.iter().any(|id| id == "old-messages"))
        .collect::<Vec<_>>();
    assert!(!old.is_empty());
    assert!(old.iter().all(
        |route| route.adapter.upstream_protocol(route.wire_api).wire_api() == WireApi::Messages
    ));
    visible.store(true, Ordering::SeqCst);
    let recovered = request_models(&state, &source.id, true).await.unwrap();
    assert_eq!(recovered.models, ["new-responses"]);
    assert_eq!(recovered.last_test_status.as_deref(), Some("ok"));
    assert!(recovered.last_error.is_none());
    assert!(!recovered.detected_model_prices.contains_key("old-messages"));
    assert!(
        recovered
            .protocol_config
            .capabilities
            .iter()
            .all(|row| row.model_id != "old-messages")
    );
    assert_eq!(state.store().unwrap().source(&source.id), Some(&recovered));
    secret_store::delete(&source.secret_ref).unwrap();
    server.abort();
    cleanup(state).await;
}

#[tokio::test]
async fn paged_model_refresh_commits_only_after_every_page_is_valid() {
    for complete in [true, false] {
        let state = state();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let (base_url, server) = serve(Router::new().route(
            "/v1/models",
            get(move |uri: axum::http::Uri| {
                let observed = observed.clone();
                async move {
                    observed.fetch_add(1, Ordering::SeqCst);
                    let second = uri.query().is_some();
                    if second && !complete {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(json!({"error": "synthetic failure"})),
                        )
                    } else {
                        let id = if second { "new-second" } else { "new-first" };
                        (
                            StatusCode::OK,
                            Json(json!({
                                "data": [{"id": id, "supported_endpoint_types": ["messages"],
                                    "inputMicroUsdPerMillion": 33, "outputMicroUsdPerMillion": 44}],
                                "has_more": !second, "last_id": "next"
                            })),
                        )
                    }
                }
            }),
        ))
        .await;
        let mut source = source_with_catalog(&base_url);
        source.wire_api = WireApi::Messages;
        source.protocol_config.endpoint_hint = Some(WireApi::Messages);
        source.protocol_bindings = source
            .protocol_config
            .resolve(&source.base_url, &source.models, &[], source.wire_api)
            .unwrap()
            .into_iter()
            .filter(|binding| binding.wire_api == WireApi::Messages)
            .collect();
        assert_eq!(source.protocol_bindings.len(), 1);
        state
            .store()
            .unwrap()
            .upsert_source(source.clone())
            .unwrap();
        secret_store::save(&source.secret_ref, "synthetic-key").unwrap();
        let before = state.store().unwrap().source(&source.id).unwrap().clone();
        let updated = request_models(&state, &source.id, true).await.unwrap();
        if complete {
            assert_eq!(updated.last_test_status.as_deref(), Some("ok"));
            assert_eq!(updated.models, ["new-first", "new-second"]);
            assert_eq!(updated.detected_model_prices.len(), 2);
            assert!(updated.detected_model_prices.contains_key("new-first"));
            assert!(updated.detected_model_prices.contains_key("new-second"));
            assert!(!updated.detected_model_prices.contains_key("test"));
            let effective = updated
                .protocol_config
                .effective_capabilities(&updated.base_url, &updated.models);
            for model in &updated.models {
                assert!(effective.iter().any(|row| {
                    row.model_id == *model
                        && row.upstream_wire_api == WireApi::Messages
                        && row.status.available()
                }));
            }
        } else {
            assert_eq!(updated.last_test_status.as_deref(), Some("error"));
            assert_eq!(updated.models, before.models);
            assert_eq!(updated.protocol_bindings, before.protocol_bindings);
            assert_eq!(updated.protocol_config, before.protocol_config);
            assert_eq!(updated.detected_model_prices, before.detected_model_prices);
        }
        assert_eq!(requests.load(Ordering::SeqCst), 2);
        assert_eq!(state.store().unwrap().source(&source.id), Some(&updated));
        secret_store::delete(&source.secret_ref).unwrap();
        server.abort();
        cleanup(state).await;
    }
}

#[tokio::test]
async fn malformed_or_failed_model_refresh_preserves_catalog_but_valid_empty_refresh_clears_it() {
    for (status, payload, valid_empty) in [
        (StatusCode::OK, json!({"data": [null]}), false),
        (
            StatusCode::OK,
            json!({"data": [{"id": "replacement"}, {}]}),
            false,
        ),
        (StatusCode::OK, json!({"data": [{"id": "   "}]}), false),
        (StatusCode::OK, json!({"error": "synthetic failure"}), false),
        (StatusCode::SERVICE_UNAVAILABLE, json!({"data": []}), false),
        (StatusCode::OK, json!({"data": []}), true),
    ] {
        let state = state();
        let (base_url, server) = serve(Router::new().route(
            "/v1/models",
            get(move || {
                let payload = payload.clone();
                async move { (status, Json(payload)) }
            }),
        ))
        .await;
        let source = source_with_catalog(&base_url);
        state
            .store()
            .unwrap()
            .upsert_source(source.clone())
            .unwrap();
        secret_store::save(&source.secret_ref, "synthetic-key").unwrap();
        let before = state.store().unwrap().source(&source.id).unwrap().clone();
        let updated = request_models(&state, &source.id, true).await.unwrap();
        assert_eq!(updated.base_url, before.base_url);
        assert_eq!(updated.enabled, before.enabled);
        assert_eq!(updated.in_pool, before.in_pool);
        if valid_empty {
            assert_eq!(updated.last_test_status.as_deref(), Some("ok"));
            assert!(updated.last_error.is_none());
            assert!(updated.models.is_empty());
            assert!(updated.detected_model_prices.is_empty());
            assert!(
                updated
                    .protocol_config
                    .resolve(
                        &updated.base_url,
                        &updated.models,
                        &updated.protocol_bindings,
                        updated.wire_api,
                    )
                    .unwrap()
                    .is_empty()
            );
        } else {
            assert_eq!(updated.last_test_status.as_deref(), Some("error"));
            assert!(updated.last_error.is_some());
            assert_eq!(updated.models, before.models);
            assert_eq!(updated.protocol_bindings, before.protocol_bindings);
            assert_eq!(updated.protocol_config, before.protocol_config);
            assert_eq!(updated.detected_model_prices, before.detected_model_prices);
        }
        assert_eq!(state.store().unwrap().source(&source.id), Some(&updated));
        secret_store::delete(&source.secret_ref).unwrap();
        server.abort();
        cleanup(state).await;
    }
}

#[tokio::test]
async fn model_refresh_preserves_manual_endpoint_and_feature_exclusions_in_saved_sources() {
    for manual_status in [CapabilityStatus::Unsupported, CapabilityStatus::Unknown] {
        let state = state();
        let (base_url, server) = serve(Router::new().route(
            "/v1/models",
            get(|| async {
                Json(json!({
                    "data": [{
                        "id": "test",
                        "supported_endpoint_types": ["responses", "messages"]
                    }]
                }))
            }),
        ))
        .await;
        let mut source = source(&base_url);
        source.protocol_config = serde_json::from_value(json!({
            "capabilities": [{
                "modelId": "test",
                "upstreamWireApi": "messages",
                "status": manual_status,
                "origin": "manual",
                "checkedAtMs": 1,
                "features": {"reasoning": "unsupported"}
            }]
        }))
        .unwrap();
        state
            .store()
            .unwrap()
            .upsert_source(source.clone())
            .unwrap();
        secret_store::save(&source.secret_ref, "synthetic-key").unwrap();
        let updated = request_models(&state, &source.id, true).await.unwrap();
        assert_eq!(updated.last_test_status.as_deref(), Some("ok"));
        assert!(updated.last_error.is_none());
        assert_eq!(updated.models, vec!["test"]);
        assert!(
            updated.protocol_config.capabilities.iter().any(|row| {
                row.origin == CapabilityOrigin::Manual && row.status == manual_status
            })
        );
        let effective = updated
            .protocol_config
            .effective_capabilities(&updated.base_url, &updated.models);
        let messages = effective
            .iter()
            .find(|row| row.upstream_wire_api == WireApi::Messages)
            .unwrap();
        assert_eq!(
            messages.status,
            if manual_status == CapabilityStatus::Unknown {
                CapabilityStatus::Declared
            } else {
                CapabilityStatus::Unsupported
            }
        );
        assert_eq!(
            messages.features.get(&ProtocolFeature::Reasoning),
            Some(&CapabilityStatus::Unsupported)
        );
        let routes = updated
            .protocol_config
            .resolve(
                &updated.base_url,
                &updated.models,
                &updated.protocol_bindings,
                updated.wire_api,
            )
            .unwrap();
        assert!(!routes.is_empty());
        assert_eq!(
            routes.iter().any(|route| {
                route.adapter.upstream_protocol(route.wire_api).wire_api() == WireApi::Messages
            }),
            manual_status == CapabilityStatus::Unknown
        );
        assert_eq!(state.store().unwrap().source(&source.id), Some(&updated));
        secret_store::delete(&source.secret_ref).unwrap();
        server.abort();
        cleanup(state).await;
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
