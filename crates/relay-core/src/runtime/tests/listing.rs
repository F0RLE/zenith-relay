use super::*;

#[test]
fn provider_image_metadata_does_not_change_unknown_model_capabilities() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    runtime.remember_codex_model_manifest("account-1", serde_json::json!({"models":[
        {"slug":"gpt-test", "input_modalities":["text"], "supported_reasoning_levels":[{"effort":"high","description":"high"}]}
    ]}), current_time_ms());
    let capabilities = runtime.model_capabilities("gpt-test");
    assert_eq!(capabilities.input_modalities, ["text"]);
    assert!(capabilities.reasoning_effort_levels.is_empty());
    assert_eq!(capabilities.tool_call, None);
}

#[test]
fn cached_native_context_windows_belong_to_the_exact_model_and_account() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    runtime.remember_codex_model_manifest(
        "account-1",
        serde_json::json!({
            "models": [
                {
                    "slug": "gpt-test",
                    "context_window": 272_000,
                    "max_context_window": 872_000
                },
                {
                    "slug": "gpt-medium",
                    "context_window": 128_000,
                    "max_context_window": 256_000
                },
                {
                    "slug": "gpt-short",
                    "context_window": 272_000
                }
            ]
        }),
        current_time_ms(),
    );

    for (model, default_tokens, maximum_tokens) in [
        ("GPT-TEST", Some(272_000), Some(872_000)),
        ("gpt-medium", Some(128_000), Some(256_000)),
        ("gpt-short", Some(272_000), None),
        ("missing", None, None),
    ] {
        assert_eq!(
            runtime.model_context_windows(model, ["account-1"]),
            crate::catalog::context_policy::ModelContextWindows {
                default_tokens,
                maximum_tokens,
            }
        );
    }
    assert_eq!(
        runtime.model_context_windows("gpt-test", ["missing-account"]),
        Default::default()
    );
}
#[test]
fn client_reasoning_projection_respects_source_scope_and_adapter_limits() {
    let sources = [
        ("messages", WireApi::Messages),
        ("native", WireApi::Responses),
    ]
    .map(|(id, upstream)| {
        let mut configured = RuntimeSource::unrestricted(source(id, "synthetic", &["test"]));
        configured.protocol_config.capabilities = vec![ModelEndpointCapability {
            model_id: "test".into(),
            upstream_wire_api: upstream,
            status: CapabilityStatus::Declared,
            origin: CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: BTreeMap::new(),
            reasoning_efforts: vec!["low".into(), "xhigh".into()],
            service_tiers: None,
        }];
        configured
    });
    let runtime = GatewayRuntime::from_pool(
        sources.into(),
        vec![RuntimeLocalKey {
            source_ids: Some(vec!["messages".into()]),
            ..RuntimeLocalKey::unrestricted(key("key", "synthetic-pool"))
        }],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(crate::model_metadata::ModelMetadataCatalogHandle::new(
                crate::model_metadata::ModelMetadataCatalog::from_models_dev_json(
                    r#"{"test/test":{"reasoning":true,"reasoning_effort_levels":["low","xhigh"]}}"#,
                )
                .unwrap(),
            )),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer synthetic-pool")))
        .unwrap();
    assert_eq!(
        runtime.client_reasoning_levels(&key, "test", WireApi::Responses),
        ["low", "xhigh"]
    );
    assert_eq!(
        runtime.client_reasoning_levels(&key, "test", WireApi::Messages),
        ["low", "xhigh"]
    );
    runtime.update_key_scope(
        "key",
        CandidateScope {
            source_ids: Some(BTreeSet::from(["native".into()])),
            account_ids: None,
            ..CandidateScope::default()
        },
    );
    assert_eq!(
        runtime.client_reasoning_levels(&key, "test", WireApi::Responses),
        ["low", "xhigh"]
    );
    assert!(runtime
        .client_reasoning_levels(&key, "missing", WireApi::Responses)
        .is_empty());
}
#[test]
fn websocket_transport_capability_is_model_scoped_and_expires() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "provider",
            &["model-a"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authorization = AuthorizationIncarnation::Source;
    runtime.mark_websocket_http_only("source", "model-a", &authorization, 1_000);
    assert!(runtime.websocket_is_http_only("source", "model-a", &authorization, 1_001));
    assert!(!runtime.websocket_is_http_only("source", "model-b", &authorization, 1_001));
    assert!(!runtime.websocket_is_http_only(
        "source",
        "model-a",
        &authorization,
        1_000 + WEBSOCKET_CAPABILITY_TTL_MS
    ));
    runtime.mark_websocket_supported("source", "model-a", &authorization);
    assert!(!runtime.websocket_is_http_only("source", "model-a", &authorization, 1_001));
}
#[test]
fn messages_source_models_are_automatically_available_to_responses_clients() {
    let mut provider = source("anthropic-source", "provider-secret", &["claude-test"]);
    provider.wire_api = WireApi::Messages;
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(provider)],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    assert_eq!(
        runtime
            .visible_models_for_secret("local-secret", &[WireApi::Responses], current_time_ms(),),
        ["claude-test"]
    );
    assert_eq!(
        runtime.visible_models_for_secret("local-secret", &[WireApi::Messages], current_time_ms(),),
        ["claude-test"]
    );

    let route_ids = runtime
        .candidate_runtime_order()
        .into_iter()
        .map(|route| route.candidate_id)
        .collect::<Vec<_>>();
    assert_eq!(route_ids, vec!["anthropic-source"]);
    let scheduler = runtime.lock_scheduler();
    let candidate = scheduler
        .candidate("anthropic-source")
        .expect("source candidate");
    assert_eq!(
        candidate.model_protocols["claude-test"],
        BTreeSet::from([
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
            WireApi::Gemini,
        ])
    );
}
#[tokio::test]
async fn source_capability_failure_does_not_permanently_hide_a_declared_model() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();
    runtime.apply_usage_event(
        &UsageEvent {
            request_id: "request".into(),
            attempt: 1,
            local_key_id: "key-1".into(),
            source_id: "source-1".into(),
            candidate_id: Some("source-1".into()),
            account_id: None,
            account_token_generation: None,
            client_context_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            wire_api: WireApi::Responses,
            transport: crate::UsageTransport::Http,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: false,
            http_status: StatusCode::BAD_REQUEST.as_u16(),
            error_category: Some("upstream_model_not_found".into()),
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: Some("gpt-test".into()),
            retry_at_ms: Some(now_ms.saturating_add(60_000)),
            consecutive_failures: Some(1),
            latency_ms: 1,
            ttft_ms: None,
            generation_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            cache_write_input_tokens: None,
            cache_write_ttl: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
            upstream_error: None,
            quota_snapshot: None,
            image_usage: None,
        },
        now_ms,
    );

    let selection = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (None, None),
            now_ms,
        )
        .await;
    assert!(selection.is_some());
}
#[test]
fn source_connector_preserves_normalized_binding_and_model_order() {
    let source = source(
        "source-1",
        "upstream-secret",
        &[
            "gpt-5.6-sol",
            "claude-opus-5",
            "gpt-5.4-mini",
            "claude-sonnet-5",
        ],
    );
    let bindings = normalize_source_protocol_bindings(
        vec![
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-opus-5".into(), "claude-sonnet-5".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-5.6-sol".into(), "gpt-5.4-mini".into()],
            },
        ],
        source.wire_api,
        &source.models,
    )
    .unwrap();

    let connector = SourceConnector::new(&source, &bindings).unwrap();

    assert_eq!(connector.protocol_bindings(), bindings.as_slice());
    assert_eq!(
        connector
            .canonical_model_for(bindings[1].key(), "GPT-5.4-MINI")
            .as_deref(),
        Some("gpt-5.4-mini")
    );
    assert!(!connector
        .protocol_bindings()
        .iter()
        .any(|binding| binding.wire_api == WireApi::ChatCompletions));
}
#[test]
fn reference_speed_choices_survive_temporary_route_health_changes() {
    let model = "gpt-future-synthetic";
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-1", "synthetic-one", &[model])),
            RuntimeSource::unrestricted(source("source-2", "synthetic-two", &[model])),
        ]
        .into_iter()
        .map(|mut source| {
            source.protocol_config.endpoint_hint = Some(WireApi::Responses);
            source
        })
        .collect(),
        vec![RuntimeLocalKey::unrestricted(key(
            "key-1",
            "synthetic-pool",
        ))],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(crate::model_metadata::ModelMetadataCatalogHandle::new(
                crate::model_metadata::ModelMetadataCatalog::from_models_dev_json(
                    r#"{"gpt-future-synthetic":{"service_tiers":["priority","ultrafast"]}}"#,
                )
                .unwrap(),
            )),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let tiers = [
        DefaultServiceTier::Standard,
        DefaultServiceTier::Fast,
        DefaultServiceTier::Ultrafast,
    ];
    assert_eq!(runtime.model_supported_service_tiers(model), tiers);
    runtime
        .set_model_service_tier_overrides(BTreeMap::from([(
            model.into(),
            DefaultServiceTier::Ultrafast,
        )]))
        .unwrap();
    runtime.set_candidate_cooldown("source-1", model, current_time_ms() + 60_000);
    runtime.remove_candidate("source-2");
    assert_eq!(
        runtime.model_effective_service_tier(model),
        DefaultServiceTier::Ultrafast
    );
    assert_eq!(runtime.model_supported_service_tiers(model), tiers);
    assert_eq!(
        runtime.model_supported_service_tiers("claude-synthetic"),
        [DefaultServiceTier::Standard]
    );
}
#[test]
fn standard_tier_shortcut_matches_route_projection_after_catalog_and_membership_changes() {
    use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};

    let model = "gpt-future-synthetic";
    let catalog = ModelMetadataCatalogHandle::new(ModelMetadataCatalog::empty());
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "synthetic",
            &[model],
        ))],
        vec![RuntimeLocalKey::unrestricted(key(
            "key-1",
            "synthetic-pool",
        ))],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(catalog.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    for payload in [
        r#"{"gpt-future-synthetic":{"reasoning":false}}"#,
        r#"{"gpt-future-synthetic":{"service_tiers":["priority"]}}"#,
        r#"{"gpt-future-synthetic":{"service_tiers":["priority","ultrafast"]}}"#,
        r#"{"gpt-future-synthetic":{"reasoning":false}}"#,
    ] {
        catalog.replace(ModelMetadataCatalog::from_models_dev_json(payload).unwrap());
        for lookup in [model, "MODEL-NOT-CONFIGURED"] {
            let scope = CandidateScope::default();
            let ids = runtime
                .lock_scheduler()
                .candidates()
                .filter(|candidate| candidate.is_configured(lookup, &WireApi::ALL_NATIVE, &scope))
                .map(|candidate| candidate.id.clone())
                .collect::<Vec<_>>();
            let routes = ids
                .iter()
                .filter_map(|id| {
                    runtime.executor_route(id, lookup, &scope, &WireApi::ALL_NATIVE, false)
                })
                .collect::<Vec<_>>();
            let expected = crate::catalog::model_service_tiers(
                lookup,
                &runtime.model_capabilities(lookup).service_tiers,
            )
            .into_iter()
            .filter(|tier| {
                *tier == DefaultServiceTier::Standard
                    || routes
                        .iter()
                        .any(|route| GatewayRuntime::route_supports_service_tier(route, *tier))
            })
            .collect::<Vec<_>>();
            assert_eq!(runtime.model_supported_service_tiers(lookup), expected);
        }
    }
    runtime.remove_candidate("source-1");
    assert_eq!(
        runtime.model_supported_service_tiers(model),
        [DefaultServiceTier::Standard]
    );
}

#[test]
fn legacy_image_base_model_configuration_remains_readable() {
    assert_eq!(
        normalize_image_base_model(Some(" auto ".into())).unwrap(),
        None
    );
    assert_eq!(
        normalize_image_base_model(Some("gpt-legacy".into()))
            .unwrap()
            .as_deref(),
        Some("gpt-legacy")
    );
}
#[test]
fn key_scope_and_prefix_filter_visible_models_without_scope_escalation() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "a", &["gpt-a"])),
            RuntimeSource::unrestricted(source("source-b", "b", &["gpt-b"])),
        ],
        vec![RuntimeLocalKey {
            key: key("key", "secret"),
            enabled: true,
            source_ids: Some(vec!["source-a".into()]),
            allowed_models: vec!["gpt-*".into()],
            excluded_models: vec!["gpt-b".into()],
            model_prefix: Some("team".into()),
        }],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    assert_eq!(
        runtime.visible_models(&authenticated, &[WireApi::Responses], current_time_ms()),
        vec!["team/gpt-a"]
    );
    assert_eq!(
        runtime.visible_models_for_secret("secret", &[WireApi::Responses], current_time_ms()),
        vec!["team/gpt-a"]
    );
    assert!(runtime
        .visible_models_for_secret("wrong", &[WireApi::Responses], current_time_ms())
        .is_empty());
    assert_eq!(
        runtime
            .resolve_model(&authenticated, "TEAM/gpt-a")
            .as_deref(),
        Some("gpt-a")
    );
}
#[test]
fn codex_aliases_resolve_without_shadowing_exact_model_ids() {
    let encoded = crate::codex_model_alias("vendor/model");
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "upstream-secret",
            &["vendor/model", &encoded],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();

    assert_eq!(
        runtime
            .resolve_visible_model(
                &authenticated,
                &encoded,
                &[WireApi::Responses],
                current_time_ms(),
            )
            .as_deref(),
        Some(encoded.as_str())
    );

    let alias = crate::codex_model_alias("vendor/model");
    let without_collision = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "upstream-secret",
            &["vendor/model"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = without_collision
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    assert_eq!(
        without_collision
            .resolve_visible_model(
                &authenticated,
                &alias,
                &[WireApi::Responses],
                current_time_ms(),
            )
            .as_deref(),
        Some("vendor/model")
    );
}
#[test]
fn configured_model_resolution_accepts_temporary_health_outage_but_not_unknown_models() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    assert!(runtime.set_candidate_health("source", CandidateHealth::Unhealthy));

    assert!(runtime
        .resolve_visible_model(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            current_time_ms(),
        )
        .is_none());
    assert_eq!(
        runtime
            .resolve_configured_model(&authenticated, "gpt-test", &[WireApi::Responses])
            .as_deref(),
        Some("gpt-test")
    );
    assert!(runtime
        .resolve_configured_model(&authenticated, "unknown", &[WireApi::Responses])
        .is_none());
}
#[test]
fn global_hidden_models_apply_to_listing_and_requests() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "a",
            &["gpt-new", "gpt-old"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions {
            hidden_models: vec!["GPT-OLD".into()],
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();

    assert_eq!(
        runtime.visible_models(&authenticated, &[WireApi::Responses], current_time_ms()),
        vec!["gpt-new"]
    );
    assert!(runtime.resolve_model(&authenticated, "gpt-old").is_none());
}
