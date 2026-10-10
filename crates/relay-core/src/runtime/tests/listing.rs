use super::*;

#[test]
fn provider_image_metadata_does_not_change_unknown_model_capabilities() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    runtime.remember_codex_model_manifest("account-1", serde_json::json!({"models":[
        {"slug":"gpt-test", "input_modalities":["text"], "supported_reasoning_levels":[{"effort":"high","description":"high"}]}
    ]}), current_time_ms());
    let capabilities = runtime.model_capabilities("gpt-test");
    assert_eq!(capabilities.input_modalities, ["text", "image"]);
    assert!(capabilities.reasoning_effort_levels.is_empty());
    assert_eq!(capabilities.tool_call, Some(true));
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
        ["low"]
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
    runtime.mark_websocket_http_only("source", "model-a", 1_000);
    assert!(runtime.websocket_is_http_only("source", "model-a", 1_001));
    assert!(!runtime.websocket_is_http_only("source", "model-b", 1_001));
    assert!(!runtime.websocket_is_http_only(
        "source",
        "model-a",
        1_000 + WEBSOCKET_CAPABILITY_TTL_MS
    ));
    runtime.mark_websocket_supported("source", "model-a");
    assert!(!runtime.websocket_is_http_only("source", "model-a", 1_001));
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
    assert!(route_ids.iter().any(|id| id == "anthropic-source"));
    assert!(route_ids
        .iter()
        .any(|id| id == "anthropic-source::responses_to_messages"));
}
#[test]
fn automatic_responses_lite_requires_every_configured_route_to_confirm_support() {
    let account_only = quota_runtime(QuotaSnapshot::default());
    let account_only_key = account_only
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    assert!(
        !account_only.codex_model_responses_routes_all_support_lite(&account_only_key, "gpt-test")
    );
    account_only.set_codex_model_uses_responses_lite("account-1", "gpt-test", true);
    assert!(
        account_only.codex_model_responses_routes_all_support_lite(&account_only_key, "gpt-test")
    );

    let first = quota_account(QuotaSnapshot::default());
    let mut second = first.clone();
    second.id = "account-2".to_string();
    second.chatgpt_account_id = "account-2".to_string();
    let all_accounts = GatewayRuntime::from_mixed_pool(
        Vec::new(),
        vec![first, second],
        vec![RuntimeMixedLocalKey {
            key: key("key-1", "local-secret"),
            enabled: true,
            source_ids: None,
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            wire_apis: None,
        }],
        RuntimeChatGptAuth {
            token_authority: Arc::new(TokenAuthority::new(1).unwrap()),
            refresh_adapter: Arc::new(NeverRefresh),
            persistence_adapter: Arc::new(NoopPersistence),
            refresh_skew_ms: 60_000,
            agent_identities: HashMap::new(),
        },
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let all_accounts_key = all_accounts
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    all_accounts.set_codex_model_uses_responses_lite("account-1", "gpt-test", true);
    assert!(
        !all_accounts.codex_model_responses_routes_all_support_lite(&all_accounts_key, "gpt-test")
    );
    all_accounts.set_codex_model_uses_responses_lite("account-2", "gpt-test", true);
    assert!(
        all_accounts.codex_model_responses_routes_all_support_lite(&all_accounts_key, "gpt-test")
    );
    assert!(all_accounts
        .codex_model_account_responses_routes_all_support_lite(&all_accounts_key, "gpt-test"));

    let mixed = GatewayRuntime::from_mixed_pool(
        vec![RuntimeSource::unrestricted(source(
            "api-source",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![quota_account(QuotaSnapshot::default())],
        vec![RuntimeMixedLocalKey {
            key: key("key-1", "local-secret"),
            enabled: true,
            source_ids: None,
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            wire_apis: None,
        }],
        RuntimeChatGptAuth {
            token_authority: Arc::new(TokenAuthority::new(1).unwrap()),
            refresh_adapter: Arc::new(NeverRefresh),
            persistence_adapter: Arc::new(NoopPersistence),
            refresh_skew_ms: 60_000,
            agent_identities: HashMap::new(),
        },
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let mixed_key = mixed
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    mixed.set_codex_model_uses_responses_lite("account-1", "gpt-test", true);
    assert!(!mixed.codex_model_responses_routes_all_support_lite(&mixed_key, "gpt-test"));
    assert!(mixed.codex_model_account_responses_routes_all_support_lite(&mixed_key, "gpt-test"));
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
fn speed_choices_and_preferences_are_independent_of_source_metadata_and_health() {
    let model = "gpt-future-synthetic";
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-1", "synthetic-one", &[model])),
            RuntimeSource::unrestricted(source("source-2", "synthetic-two", &[model])),
        ],
        vec![RuntimeLocalKey::unrestricted(key(
            "key-1",
            "synthetic-pool",
        ))],
        GatewayRuntimeOptions::default(),
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
fn image_main_model_prefers_cheapest_tier_without_model_name_allowlist() {
    let models = normalized_set(
        [
            "gpt-5.6-terra".to_string(),
            "gpt-5.6-sol".to_string(),
            "gpt-5.4-mini".to_string(),
        ]
        .iter()
        .collect::<Vec<_>>(),
    );
    // Automatic selection uses the immutable LiteLLM snapshot when one is
    // available.  Keep the fixture explicit so this test does not depend on
    // the shared LiteLLM fixture catalog.
    let catalog = crate::pricing::PricingCatalog::from_litellm_json(
        r#"{
            "gpt-5.6-terra": {
                "litellm_provider": "openai",
                "input_cost_per_token": "0.000002",
                "output_cost_per_token": "0.000012"
            },
            "gpt-5.6-sol": {
                "litellm_provider": "openai",
                "input_cost_per_token": "0.000004",
                "output_cost_per_token": "0.000020"
            },
            "gpt-5.4-mini": {
                "litellm_provider": "openai",
                "input_cost_per_token": "0.000001",
                "output_cost_per_token": "0.000006"
            }
        }"#,
    )
    .unwrap();
    assert_eq!(
        super::images::cheapest_image_main_model_with_catalog(&models, Some(&catalog)).as_deref(),
        Some("gpt-5.4-mini")
    );
    // An empty/offline snapshot must still allow a deterministic runtime
    // build; its choice is a stable fallback, not an implicit price claim.
    let empty = crate::pricing::PricingCatalog::empty();
    assert_eq!(
        super::images::cheapest_image_main_model_with_catalog(&models, Some(&empty)).as_deref(),
        Some("gpt-5.6-sol")
    );
    let terra = normalized_set(["gpt-5.6-terra".to_string()].iter());
    assert_eq!(
        cheapest_image_main_model(&terra).as_deref(),
        Some("gpt-5.6-terra")
    );
    let image = normalized_set([IMAGE_API_MODEL.to_string()].iter());
    assert!(cheapest_image_main_model(&image).is_none());
}
#[test]
fn explicit_image_base_model_is_used_only_when_available() {
    let models = normalized_set(
        ["gpt-5.4-mini".to_string(), "gpt-5.6-sol".to_string()]
            .iter()
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        select_image_main_model(&models, Some("gpt-5.6-sol")).as_deref(),
        Some("gpt-5.6-sol")
    );
    assert_eq!(select_image_main_model(&models, Some("future-model")), None);
    let future = normalized_set(["gpt-future".to_string()].iter());
    assert!(cheapest_image_main_model(&future).is_none());
    assert_eq!(
        select_image_main_model(&future, Some("gpt-future")).as_deref(),
        Some("gpt-future")
    );
    let legacy = normalized_set(["gpt-4.1-mini".to_string()].iter());
    assert!(cheapest_image_main_model(&legacy).is_none());
    assert_eq!(
        normalize_image_base_model(Some(" auto ".into())).unwrap(),
        None
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
