use super::*;
use axum::http::{HeaderMap, StatusCode};
use serde_json::json;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

struct Fixture {
    source: ProviderSource,
    bindings: Vec<SourceProtocolBinding>,
    config: SourceProtocolConfig,
    prices: BTreeMap<String, ApiModelPriceOverride>,
    messages_visible: Arc<AtomicBool>,
    server: tokio::task::JoinHandle<()>,
}

impl Fixture {
    fn apply(&mut self, discovery: &SourceDiscovery) {
        discovery.apply_catalog(
            &mut self.source.base_url,
            &mut self.source.models,
            &mut self.bindings,
            &mut self.config,
            &mut self.prices,
        );
    }
}

async fn fixture(with_history: bool) -> Fixture {
    let messages_visible = Arc::new(AtomicBool::new(false));
    let visible = messages_visible.clone();
    let router = Router::new().route(
        "/v1/models",
        get(move |headers: HeaderMap| {
            let visible = visible.clone();
            async move {
                if headers.contains_key("x-api-key") && !visible.load(Ordering::SeqCst) {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(json!({"error": "synthetic failure"})),
                    );
                }
                let (id, endpoint) = if headers.contains_key("x-api-key") {
                    ("new-messages", "messages")
                } else {
                    ("new-responses", "responses")
                };
                (
                    StatusCode::OK,
                    Json(json!({"data": [{"id": id,
                    "supported_endpoint_types": [endpoint],
                    "inputMicroUsdPerMillion": 33, "outputMicroUsdPerMillion": 44}]})),
                )
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let responses = if with_history {
        vec!["old-responses".into()]
    } else {
        vec![]
    };
    let messages = if with_history {
        vec!["old-messages".into()]
    } else {
        vec![]
    };
    let models = responses.iter().chain(&messages).cloned().collect();
    let bindings = vec![
        SourceProtocolBinding::legacy(WireApi::Responses, &responses),
        SourceProtocolBinding::legacy(WireApi::Messages, &messages),
    ];
    let config = if with_history {
        serde_json::from_value(json!({"capabilities": [
            {"modelId": "old-messages", "upstreamWireApi": "messages", "status": "declared",
                "origin": "catalog", "checkedAtMs": 1, "features": {"reasoning": "unsupported"}},
            {"modelId": "old-messages", "upstreamWireApi": "responses", "status": "unsupported",
                "origin": "catalog", "checkedAtMs": 1}
        ]}))
        .unwrap()
    } else {
        SourceProtocolConfig::default()
    };
    let price = ApiModelPriceOverride {
        input_micro_usd_per_million: 11,
        output_micro_usd_per_million: 22,
        ..Default::default()
    };
    let prices = if with_history {
        BTreeMap::from([
            ("old-responses".into(), price),
            ("old-messages".into(), price),
        ])
    } else {
        BTreeMap::new()
    };
    Fixture {
        source: ProviderSource {
            id: "synthetic-partial".into(),
            name: "Synthetic partial source".into(),
            base_url: format!("http://{address}/v1"),
            api_key: "synthetic-key".into(),
            wire_api: WireApi::Responses,
            models,
            proxy_url: None,
        },
        bindings,
        config,
        prices,
        messages_visible,
        server,
    }
}

#[tokio::test]
async fn partial_refresh_preserves_only_the_failed_scopes_previous_inventory_and_evidence() {
    let mut fixture = fixture(true).await;
    let discovery = read_source_models(&fixture.source, &fixture.bindings, &fixture.config)
        .await
        .read_value
        .unwrap();
    assert_eq!(discovery.failures.len(), 1);
    assert!(discovery.failure_summary().unwrap().contains("403"));
    assert_eq!(
        discovery.models_with_previous(&fixture.source.models),
        ["new-responses", "old-messages"]
    );
    assert_eq!(
        discovery.models_with_previous(&["old-messages".into(), "OLD-MESSAGES".into()]),
        ["new-responses", "old-messages"]
    );
    fixture.apply(&discovery);
    assert_eq!(fixture.source.models, ["new-responses", "old-messages"]);
    assert_eq!(fixture.bindings.len(), 2);
    assert_eq!(fixture.prices.len(), 2);
    assert!(!fixture.prices.contains_key("old-responses"));
    assert_eq!(
        fixture.prices["old-messages"].input_micro_usd_per_million,
        11
    );
    assert_eq!(
        fixture.prices["new-responses"].input_micro_usd_per_million,
        33
    );
    let effective = fixture
        .config
        .effective_capabilities(&fixture.source.base_url, &fixture.source.models);
    let previous = effective
        .iter()
        .find(|row| row.model_id == "old-messages" && row.upstream_wire_api == WireApi::Messages)
        .unwrap();
    assert_eq!(previous.checked_at_ms, 1);
    assert_eq!(
        previous.features.get(&crate::ProtocolFeature::Reasoning),
        Some(&CapabilityStatus::Unsupported)
    );
    let routes = fixture
        .config
        .resolve(
            &fixture.source.base_url,
            &fixture.source.models,
            &fixture.bindings,
            fixture.source.wire_api,
        )
        .unwrap();
    let old = routes
        .iter()
        .filter(|route| route.model_ids.iter().any(|id| id == "old-messages"))
        .collect::<Vec<_>>();
    assert!(!old.is_empty());
    assert!(old.iter().all(
        |route| route.adapter.upstream_protocol(route.wire_api).wire_api() == WireApi::Messages
    ));
    fixture.server.abort();
}

#[tokio::test]
async fn a_failed_empty_binding_remains_retryable_without_inheriting_other_models() {
    let mut fixture = fixture(false).await;
    let discovery = read_source_models(&fixture.source, &fixture.bindings, &fixture.config)
        .await
        .read_value
        .unwrap();
    fixture.apply(&discovery);
    assert_eq!(fixture.source.models, ["new-responses"]);
    assert_eq!(fixture.bindings.len(), 2);
    assert!(
        fixture
            .bindings
            .iter()
            .find(|binding| binding.wire_api == WireApi::Messages)
            .unwrap()
            .model_ids
            .is_empty()
    );
    let routes = fixture
        .config
        .resolve(
            &fixture.source.base_url,
            &fixture.source.models,
            &fixture.bindings,
            fixture.source.wire_api,
        )
        .unwrap();
    assert!(!routes.is_empty());
    assert!(routes.iter().all(
        |route| route.adapter.upstream_protocol(route.wire_api).wire_api() == WireApi::Responses
    ));
    fixture.messages_visible.store(true, Ordering::SeqCst);
    let recovery = read_source_models(&fixture.source, &fixture.bindings, &fixture.config)
        .await
        .read_value
        .unwrap();
    assert!(recovery.failures.is_empty());
    assert!(recovery.failure_summary().is_none());
    fixture.apply(&recovery);
    assert_eq!(fixture.source.models, ["new-responses", "new-messages"]);
    fixture.server.abort();
}

#[test]
fn overlap_keeps_unread_feature_restrictions_without_restoring_old_prices() {
    for origin in ["catalog", "manual"] {
        for status in ["declared", "unsupported"] {
            let mut config: SourceProtocolConfig =
                serde_json::from_value(json!({"capabilities": [{
                    "modelId": "shared", "upstreamWireApi": "messages", "status": status,
                    "origin": origin, "checkedAtMs": 1, "features": {"reasoning": "unsupported"},
                    "tokenLimits": {"context": 8192}
                }]}))
                .unwrap();
            let previous_limits = config.capabilities[0].token_limits;
            assert!(!previous_limits.is_empty());
            let message_binding =
                SourceProtocolBinding::legacy(WireApi::Messages, &["shared".into()]);
            let discovery = SourceDiscovery {
                models: vec!["shared".into()],
                protocol_bindings: vec![SourceProtocolBinding::legacy(WireApi::Responses, &["shared".into()])],
                capabilities: serde_json::from_value(json!([
                    {"modelId": "shared", "upstreamWireApi": "responses", "status": "declared", "origin": "catalog", "checkedAtMs": 100},
                    {"modelId": "shared", "upstreamWireApi": "messages", "status": "declared", "origin": "catalog", "checkedAtMs": 100}
                ])).unwrap(),
                catalog_models: BTreeMap::from([(WireApi::Responses, vec!["shared".into()])]),
                failures: vec![SourceCatalogFailure::new(&message_binding, &crate::Error::UpstreamStatus(403))],
                ..Default::default()
            };
            let mut base_url = "https://provider.example.test/v1".into();
            let mut models = vec!["shared".into()];
            let mut bindings = vec![message_binding];
            let mut prices = BTreeMap::from([(
                "shared".into(),
                ApiModelPriceOverride {
                    input_micro_usd_per_million: 11,
                    output_micro_usd_per_million: 22,
                    ..Default::default()
                },
            )]);
            discovery.apply_catalog(
                &mut base_url,
                &mut models,
                &mut bindings,
                &mut config,
                &mut prices,
            );
            assert!(prices.is_empty());
            let effective = config.effective_capabilities(&base_url, &models);
            let messages = effective
                .iter()
                .find(|row| row.upstream_wire_api == WireApi::Messages)
                .unwrap();
            assert_eq!(
                messages.features.get(&crate::ProtocolFeature::Reasoning),
                Some(&CapabilityStatus::Unsupported)
            );
            assert_eq!(messages.token_limits, previous_limits);
            assert_eq!(
                messages.status,
                if origin == "manual" && status == "unsupported" {
                    CapabilityStatus::Unsupported
                } else {
                    CapabilityStatus::Declared
                }
            );
        }
    }
}

#[test]
fn a_successful_read_of_the_same_physical_protocol_supersedes_failed_alias_history() {
    let mut failed = SourceProtocolBinding::legacy(WireApi::Responses, &["removed".into()]);
    failed.adapter = SourceAdapter::ResponsesToMessages;
    failed.reasoning_mode = crate::MessagesReasoningMode::Adaptive;
    let discovery = SourceDiscovery {
        models: vec!["new-model".into()],
        protocol_bindings: vec![SourceProtocolBinding::legacy(
            WireApi::Messages,
            &["new-model".into()],
        )],
        catalog_models: BTreeMap::from([(WireApi::Messages, vec!["new-model".into()])]),
        failures: vec![SourceCatalogFailure::new(
            &failed,
            &crate::Error::UpstreamStatus(503),
        )],
        ..Default::default()
    };
    assert_eq!(
        discovery.models_with_previous(&["removed".into()]),
        ["new-model"]
    );
    let mut base = "https://provider.example.test/v1".into();
    let mut models = vec!["removed".into()];
    let mut bindings = vec![failed];
    discovery.apply_catalog(
        &mut base,
        &mut models,
        &mut bindings,
        &mut SourceProtocolConfig::default(),
        &mut BTreeMap::new(),
    );
    assert_eq!(models, ["new-model"]);
    assert_eq!(bindings.len(), 2);
    assert!(
        bindings
            .iter()
            .find(|binding| binding.adapter == SourceAdapter::ResponsesToMessages)
            .unwrap()
            .model_ids
            .is_empty()
    );
}

#[test]
fn failure_summaries_redact_request_and_validation_details() {
    let binding = SourceProtocolBinding::legacy(WireApi::Messages, &["private-model-name".into()]);
    let invalid = reqwest::Client::new()
        .get("https://private-user:private-key@invalid host")
        .build()
        .unwrap_err();
    let discovery = SourceDiscovery {
        failures: vec![
            SourceCatalogFailure::new(
                &binding,
                &crate::Error::Validation("private-url private-key".into()),
            ),
            SourceCatalogFailure::new(&binding, &crate::Error::Upstream(invalid)),
            SourceCatalogFailure::new(&binding, &crate::Error::UpstreamStatus(403)),
        ],
        ..Default::default()
    };
    let summary = discovery.failure_summary().unwrap();
    assert!(summary.contains("403"));
    assert!(summary.contains("Messages"));
    assert!(!summary.contains("private-"));
    assert!(!summary.contains("http://"));
}

#[tokio::test]
async fn root_resolution_rechecks_earlier_successes_at_the_canonical_path() {
    let router = Router::new()
        .route(
            "/models",
            get(|headers: HeaderMap| async move {
                if headers.contains_key("x-api-key") {
                    (
                        StatusCode::OK,
                        Json(json!({"data": [{"id": "old-root-messages"}]})),
                    )
                } else {
                    (StatusCode::NOT_FOUND, Json(json!({"error": "not found"})))
                }
            }),
        )
        .route(
            "/v1/models",
            get(|headers: HeaderMap| async move {
                let id = if headers.contains_key("x-api-key") {
                    "canonical-messages"
                } else {
                    "canonical-responses"
                };
                Json(json!({"data": [{"id": id}]}))
            }),
        );
    let (mut source, server) =
        super::pagination::source_for_router(router, WireApi::Responses).await;
    source.base_url = source.base_url.trim_end_matches("/v1").to_string();
    let discovery = discover_source_models_and_protocol_bindings(
        &source,
        &[
            SourceProtocolBinding::legacy(WireApi::Messages, &[]),
            SourceProtocolBinding::legacy(WireApi::Responses, &[]),
        ],
    )
    .await
    .unwrap();
    assert!(discovery.failures.is_empty());
    assert_eq!(
        discovery.models,
        ["canonical-messages", "canonical-responses"]
    );
    assert_eq!(
        discovery.resolved_base_url.as_deref(),
        Some(format!("{}/v1", source.base_url).as_str())
    );
    server.abort();
}

#[tokio::test]
async fn partial_root_resolution_cannot_relocate_existing_scope_evidence() {
    let router = Router::new()
        .route("/models", get(|| async { StatusCode::NOT_FOUND }))
        .route(
            "/v1/models",
            get(|headers: HeaderMap| async move {
                if headers.contains_key("x-api-key") {
                    (
                        StatusCode::FORBIDDEN,
                        Json(json!({"error": "synthetic failure"})),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(json!({"data": [{"id": "new-responses"}]})),
                    )
                }
            }),
        );
    let (mut source, server) =
        super::pagination::source_for_router(router, WireApi::Responses).await;
    source.base_url = source.base_url.trim_end_matches("/v1").to_string();
    let result = discover_source_models_and_protocol_bindings(
        &source,
        &[
            SourceProtocolBinding::legacy(WireApi::Messages, &[]),
            SourceProtocolBinding::legacy(WireApi::Responses, &[]),
        ],
    )
    .await;
    assert!(matches!(result, Err(crate::Error::UpstreamStatus(403))));
    server.abort();
}

#[tokio::test]
async fn canonical_replay_keeps_the_bindings_remaining_read_time_budget() {
    let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
    let root_started = started.clone();
    let router = Router::new()
        .route(
            "/models",
            get(move |headers: HeaderMap| {
                let started = root_started.clone();
                async move {
                    if headers.contains_key("x-api-key") {
                        started.send(false).unwrap();
                        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                        (
                            StatusCode::OK,
                            Json(json!({"data": [{"id": "old-root-messages"}]})),
                        )
                    } else {
                        (StatusCode::NOT_FOUND, Json(json!({"error": "not found"})))
                    }
                }
            }),
        )
        .route(
            "/v1/models",
            get(move |headers: HeaderMap| {
                let started = started.clone();
                async move {
                    if headers.contains_key("x-api-key") {
                        started.send(true).unwrap();
                        tokio::time::sleep(std::time::Duration::from_secs(20)).await;
                    }
                    Json(json!({"data": [{"id": "canonical-model"}]}))
                }
            }),
        );
    let (mut source, server) =
        super::pagination::source_for_router(router, WireApi::Responses).await;
    source.base_url = source.base_url.trim_end_matches("/v1").to_string();
    let read = tokio::spawn(async move {
        discover_source_models_and_protocol_bindings(
            &source,
            &[
                SourceProtocolBinding::legacy(WireApi::Messages, &[]),
                SourceProtocolBinding::legacy(WireApi::Responses, &[]),
            ],
        )
        .await
    });
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), starts.recv())
            .await
            .unwrap(),
        Some(false)
    );
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(21)).await;
    tokio::time::resume();
    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(2), starts.recv())
            .await
            .unwrap(),
        Some(true)
    );
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(10)).await;
    tokio::time::resume();
    let result = tokio::time::timeout(std::time::Duration::from_secs(1), read)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        result,
        Err(crate::Error::ManagementHttpUnavailable)
    ));
    server.abort();
}
