use super::*;

#[tokio::test]
async fn invalid_local_key_stops_before_upstream_execution() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .bearer_auth("wrong-local-key")
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn image_generation_for_api_source_preserves_requested_image_model() {
    let state = UpstreamState::default();
    let upstream = spawn(
        Router::new()
            .route("/v1/images/generations", post(upstream_image_generation))
            .with_state(state.clone()),
    )
    .await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource {
            source: ProviderSource {
                id: "image-source".to_string(),
                name: "Synthetic image source".to_string(),
                base_url: format!("{}/v1", upstream.base_url),
                api_key: SOURCE_KEY.to_string(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-image-1.5".to_string()],
            },
            protocol_config: Default::default(),
            protocol_bindings: vec![SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-image-1.5".to_string()],
            }],
            enabled: true,
            draining: false,
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            last_used_at_ms: None,
        }],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    let gateway = spawn(gateway::router(Arc::new(runtime))).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-image-1.5","prompt":"draw a test"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["data"][0]["b64_json"],
        "aW1hZ2U="
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/images/generations");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
    let bodies = state.bodies.lock().unwrap();
    assert_eq!(bodies[0]["model"], "gpt-image-1.5");
    assert_eq!(bodies[0]["prompt"], "draw a test");
    drop(bodies);
    assert!(events.lock().unwrap()[0].success);
}

#[tokio::test]
async fn non_local_host_stops_before_auth_and_upstream_execution() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .get(format!("{}/v1/models", gateway.base_url))
        .header(HOST, "example.test")
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::MISDIRECTED_REQUEST);
    assert!(state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn models_are_discovered_with_the_source_credential() {
    let (upstream, state) = spawn_upstream().await;
    let models = discover_source_models(&ProviderSource {
        id: "source-1".into(),
        name: "Synthetic upstream".into(),
        base_url: format!("{}/v1", upstream.base_url),
        api_key: SOURCE_KEY.into(),
        wire_api: WireApi::Responses,
        models: vec!["gpt-test".into()],
    })
    .await
    .unwrap();
    assert_eq!(models, ["gpt-test", "hidden-model"]);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
}

#[tokio::test]
async fn root_openai_discovery_retries_v1_after_404_and_returns_resolved_url() {
    let state = UpstreamState::default();
    let root_state = state.clone();
    let v1_state = state.clone();
    let root_status = StatusCode::NOT_FOUND;
    let upstream = spawn(
        Router::new()
            .route(
                "/models",
                get(move |headers: HeaderMap| {
                    let state = root_state.clone();
                    async move {
                        observe(&state, "/models", &headers);
                        root_status.into_response()
                    }
                }),
            )
            .route(
                "/v1/models",
                get(move |headers: HeaderMap| {
                    let state = v1_state.clone();
                    async move { upstream_models(State(state), headers).await }
                }),
            ),
    )
    .await;

    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-root".into(),
            name: "Root OpenAI-compatible upstream".into(),
            base_url: upstream.base_url.clone(),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[],
    )
    .await
    .unwrap();

    assert_eq!(discovery.models, ["gpt-test", "hidden-model"]);
    let expected_base_url = format!("{}/v1", upstream.base_url);
    assert_eq!(
        discovery.resolved_base_url.as_deref(),
        Some(expected_base_url.as_str())
    );
    let requests = state.requests.lock().unwrap();
    assert_eq!(
        requests
            .iter()
            .map(|request| request.path)
            .collect::<Vec<_>>(),
        ["/models", "/v1/models"]
    );
}

#[tokio::test]
async fn root_openai_discovery_does_not_guess_v1_after_auth_or_rate_limit_failures() {
    for status in [
        StatusCode::UNAUTHORIZED,
        StatusCode::FORBIDDEN,
        StatusCode::TOO_MANY_REQUESTS,
    ] {
        let state = UpstreamState::default();
        let root_state = state.clone();
        let v1_state = state.clone();
        let upstream = spawn(
            Router::new()
                .route(
                    "/models",
                    get(move |headers: HeaderMap| {
                        let state = root_state.clone();
                        async move {
                            observe(&state, "/models", &headers);
                            status.into_response()
                        }
                    }),
                )
                .route(
                    "/v1/models",
                    get(move |headers: HeaderMap| {
                        let state = v1_state.clone();
                        async move { upstream_models(State(state), headers).await }
                    }),
                ),
        )
        .await;

        let result = discover_source_models_and_protocol_bindings(
            &ProviderSource {
                id: "source-root".into(),
                name: "Root OpenAI-compatible upstream".into(),
                base_url: upstream.base_url.clone(),
                api_key: SOURCE_KEY.into(),
                wire_api: WireApi::Responses,
                models: Vec::new(),
            },
            &[],
        )
        .await;

        assert!(result.is_err(), "status {status} must fail discovery");
        let requests = state.requests.lock().unwrap();
        assert_eq!(
            requests.len(),
            1,
            "status {status} must not trigger fallback"
        );
        assert_eq!(requests[0].path, "/models");
    }
}

#[tokio::test]
async fn native_messages_model_discovery_preserves_inventory_and_uses_anthropic_headers() {
    let (upstream, state) = spawn_upstream().await;
    let models = discover_source_models_for_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic Anthropic upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Messages,
            models: vec!["claude-test".into(), "claude-hidden".into()],
        },
        &[SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-test".into()],
        }],
    )
    .await
    .unwrap();
    assert_eq!(models, ["claude-test", "claude-hidden"]);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(requests[0].authorization, None);
    assert_eq!(requests[0].x_api_key.as_deref(), Some(SOURCE_KEY));
    assert_eq!(requests[0].anthropic_version.as_deref(), Some("2023-06-01"));
}

#[tokio::test]
async fn responses_to_messages_discovery_keeps_responses_client_binding() {
    let (upstream, state) = spawn_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic bridged upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        }],
    )
    .await
    .unwrap();
    assert_eq!(discovery.models, ["claude-test", "claude-hidden"]);
    assert_eq!(
        discovery.protocol_bindings,
        [SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        }]
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/models");
    assert_eq!(requests[0].authorization, None);
    assert_eq!(requests[0].x_api_key.as_deref(), Some(SOURCE_KEY));
    assert_eq!(requests[0].anthropic_version.as_deref(), Some("2023-06-01"));
}

#[tokio::test]
async fn source_wide_catalog_binding_refreshes_new_models() {
    let (upstream, _) = spawn_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic source-wide upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into()],
        },
        &[SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-test".into()],
        }],
    )
    .await
    .unwrap();

    assert_eq!(discovery.models, ["gpt-test", "hidden-model"]);
    assert_eq!(
        discovery.protocol_bindings,
        [SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        }]
    );
}

#[tokio::test]
async fn native_responses_catalog_refreshes_after_models_are_split_to_a_messages_bridge() {
    let upstream = spawn_mixed_catalog_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic mixed source-wide upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: vec!["gpt-test".into(), "claude-test".into()],
        },
        &[
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                // Legacy source-level reasoning values are normalized to the
                // bridge's technical adaptive translator at runtime.
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into()],
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        discovery.models,
        ["gpt-test", "hidden-model", "claude-test"]
    );
    assert_eq!(
        discovery.protocol_bindings,
        [
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".into(), "hidden-model".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into()],
            },
        ]
    );
}

#[tokio::test]
async fn native_model_discovery_keeps_each_protocol_catalog_separate() {
    let (upstream, state) = spawn_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic mixed upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        discovery.models,
        ["gpt-test", "hidden-model", "claude-test", "claude-hidden"]
    );
    assert_eq!(
        discovery.protocol_bindings,
        [
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".into(), "hidden-model".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into(), "claude-hidden".into()],
            },
        ]
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.authorization.is_some())
            .count(),
        1
    );
    assert_eq!(
        requests
            .iter()
            .filter(|request| request.x_api_key.is_some())
            .count(),
        1
    );
}

#[tokio::test]
async fn discovery_retains_prices_outside_legacy_binding_lists() {
    let upstream = spawn(Router::new().route(
        "/v1/models",
        get(|headers: HeaderMap| async move {
            let messages = has_messages_source_key(&headers);
            Json(json!({"data": [
                {"id": "saved"},
                {"id": "new-model", "pricing": {
                    "inputMicroUsdPerMillion": 3_000_000,
                    "outputMicroUsdPerMillion": 15_000_000,
                    "cacheWrite5mMicroUsdPerMillion": 3_750_000,
                    "cacheWrite1hMicroUsdPerMillion": 6_000_000
                }},
                {"id": "conflicting", "pricing": {
                    "inputMicroUsdPerMillion": if messages { 2_000_000 } else { 1_000_000 },
                    "outputMicroUsdPerMillion": 4_000_000
                }}
            ]}))
        }),
    ))
    .await;
    let source = ProviderSource {
        id: "source-1".into(),
        name: "Synthetic priced catalog".into(),
        base_url: format!("{}/v1", upstream.base_url),
        api_key: SOURCE_KEY.into(),
        wire_api: WireApi::Responses,
        models: Vec::new(),
    };
    let bindings = [WireApi::Responses, WireApi::Messages].map(|wire_api| SourceProtocolBinding {
        wire_api,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["saved".into()],
    });
    let discovery =
        discover_source_with_protocol_config(&source, &bindings, &SourceProtocolConfig::default())
            .await
            .unwrap();
    assert_eq!(discovery.models, ["saved", "new-model", "conflicting"]);
    let price = &discovery.detected_model_prices["new-model"];
    assert_eq!(price.input_micro_usd_per_million, 3_000_000);
    assert_eq!(price.output_micro_usd_per_million, 15_000_000);
    assert_eq!(price.cache_write_5m_micro_usd_per_million, Some(3_750_000));
    assert_eq!(price.cache_write_1h_micro_usd_per_million, Some(6_000_000));
    assert!(!discovery.detected_model_prices.contains_key("conflicting"));
}

#[tokio::test]
async fn configured_discovery_hints_union_physical_catalogs_without_filtering_runtime_routes() {
    let (upstream, _) = spawn_upstream().await;
    let source = ProviderSource {
        id: "source-1".into(),
        name: "Synthetic mixed upstream".into(),
        base_url: format!("{}/v1", upstream.base_url),
        api_key: SOURCE_KEY.into(),
        wire_api: WireApi::Responses,
        models: Vec::new(),
    };
    let hints = [
        SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        },
        SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        },
    ];

    let discovery =
        discover_source_with_protocol_config(&source, &hints, &SourceProtocolConfig::default())
            .await
            .unwrap();

    assert_eq!(
        discovery.models,
        ["gpt-test", "hidden-model", "claude-test", "claude-hidden"]
    );
    assert_eq!(discovery.protocol_bindings.len(), 2);
    let routes = SourceProtocolConfig {
        capabilities: discovery.capabilities,
        ..Default::default()
    }
    .resolve(
        &source.base_url,
        &discovery.models,
        &discovery.protocol_bindings,
        source.wire_api,
    )
    .unwrap();
    for model in &discovery.models {
        let upstream = if model.starts_with("claude-") {
            WireApi::Messages
        } else {
            WireApi::Responses
        };
        assert!(routes
            .iter()
            .filter(|route| route.model_ids.contains(model))
            .all(|route| route.adapter.upstream_protocol(route.wire_api).wire_api() == upstream));
        assert_eq!(
            routes
                .iter()
                .filter(|route| route.model_ids.contains(model))
                .count(),
            WireApi::ALL.len()
        );
    }
}

#[tokio::test]
async fn native_and_bridged_responses_discovery_keep_route_catalogs_separate() {
    let (upstream, state) = spawn_upstream().await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Synthetic mixed Responses source".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        discovery.models,
        ["gpt-test", "hidden-model", "claude-test", "claude-hidden"]
    );
    assert_eq!(
        discovery.protocol_bindings,
        [
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".into(), "hidden-model".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".into(), "claude-hidden".into()],
            },
        ]
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().any(|request| {
        request.authorization.as_deref() == Some("Bearer upstream-test-key")
            && request.x_api_key.is_none()
    }));
    assert!(requests.iter().any(|request| {
        request.authorization.is_none()
            && request.x_api_key.as_deref() == Some(SOURCE_KEY)
            && request.anthropic_version.as_deref() == Some("2023-06-01")
    }));
}

#[tokio::test]
async fn failed_native_binding_is_not_advertised_after_discovery() {
    let upstream =
        spawn(Router::new().route("/v1/models", get(upstream_models_rejecting_messages))).await;
    let discovery = discover_source_models_and_protocol_bindings(
        &ProviderSource {
            id: "source-1".into(),
            name: "Partial mixed upstream".into(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.into(),
            wire_api: WireApi::Responses,
            models: Vec::new(),
        },
        &[
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: Vec::new(),
            },
        ],
    )
    .await
    .unwrap();

    assert_eq!(discovery.models, ["gpt-test"]);
    assert_eq!(
        discovery.protocol_bindings,
        [SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-test".into()],
        }]
    );
}

#[tokio::test]
async fn model_discovery_rejects_a_bad_source_key() {
    let (upstream, _) = spawn_upstream().await;
    assert!(discover_source_models(&ProviderSource {
        id: "source-1".into(),
        name: "Synthetic upstream".into(),
        base_url: format!("{}/v1", upstream.base_url),
        api_key: "wrong-source-key".into(),
        wire_api: WireApi::Responses,
        models: vec!["gpt-test".into()],
    })
    .await
    .is_err());
}

#[tokio::test]
async fn model_discovery_rejects_an_oversized_body() {
    let upstream = spawn(Router::new().route(
        "/v1/models",
        get(|| async {
            Response::builder()
                .status(StatusCode::OK)
                .header("content-length", OVERSIZED_MODELS_CONTENT_LENGTH)
                .body(Body::empty())
                .unwrap()
        }),
    ))
    .await;
    assert!(discover_source_models(&ProviderSource {
        id: "source-1".into(),
        name: "Synthetic upstream".into(),
        base_url: format!("{}/v1", upstream.base_url),
        api_key: SOURCE_KEY.into(),
        wire_api: WireApi::Responses,
        models: vec![],
    })
    .await
    .is_err());
}
