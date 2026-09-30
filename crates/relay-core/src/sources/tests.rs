use super::*;
use std::collections::BTreeMap;
use url::Url;

#[test]
fn url_userinfo_includes_a_username_or_a_password() {
    assert!(!url_has_userinfo(
        &Url::parse("https://api.example/v1").unwrap()
    ));
    assert!(url_has_userinfo(
        &Url::parse("https://user@api.example/v1").unwrap()
    ));
    assert!(url_has_userinfo(
        &Url::parse("https://:secret@api.example/v1").unwrap()
    ));
    assert!(url_has_userinfo(
        &Url::parse("https://user:secret@api.example/v1").unwrap()
    ));
}

#[test]
fn http_endpoint_requires_http_and_a_host() {
    assert!(is_http_endpoint(
        &Url::parse("https://api.example/v1").unwrap()
    ));
    assert!(is_http_endpoint(
        &Url::parse("http://127.0.0.1:8787").unwrap()
    ));
    assert!(!is_http_endpoint(&Url::parse("ftp://api.example").unwrap()));
    assert!(!is_http_endpoint(
        &Url::parse("data:text/plain,hi").unwrap()
    ));
}

#[test]
fn mixed_empty_protocol_bindings_stay_unconfirmed() {
    let bindings = normalize_source_protocol_bindings(
        vec![
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
        WireApi::Responses,
        &["gpt-test".to_string(), "claude-test".to_string()],
    )
    .unwrap();
    assert_eq!(
        bindings,
        [
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
        ]
    );
}

#[test]
fn runtime_keeps_native_messages_without_an_implicit_responses_route() {
    let bindings = runtime_source_protocol_bindings(
        vec![SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::OneHour,
            model_ids: vec!["claude-test".to_string()],
        }],
        WireApi::Messages,
        &["claude-test".to_string()],
    )
    .unwrap();

    assert_eq!(bindings.len(), 1);
    assert_eq!(bindings[0].wire_api, WireApi::Messages);
    assert_eq!(bindings[0].adapter, SourceAdapter::Native);
    assert_eq!(bindings[0].model_ids, ["claude-test"]);
    assert_eq!(bindings[0].cache_write_ttl, CacheWriteTtl::OneHour);
}

#[test]
fn runtime_does_not_shadow_explicit_responses_or_bridge_routes() {
    let explicit = vec![
        SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            model_ids: vec!["gpt-test".to_string()],
        },
        SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            model_ids: vec!["claude-test".to_string()],
        },
    ];
    let bindings = runtime_source_protocol_bindings(
        explicit,
        WireApi::Responses,
        &["gpt-test".to_string(), "claude-test".to_string()],
    )
    .unwrap();
    assert_eq!(
        bindings
            .iter()
            .map(SourceProtocolBinding::key)
            .collect::<Vec<_>>(),
        [
            SourceProtocolBindingKey {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
            },
            SourceProtocolBindingKey {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
            },
        ]
    );

    let already_bridged = runtime_source_protocol_bindings(
        vec![
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                model_ids: vec!["claude-test".to_string()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::FiveMinutes,
                model_ids: vec!["claude-test".to_string()],
            },
        ],
        WireApi::Messages,
        &["claude-test".to_string()],
    )
    .unwrap();
    assert_eq!(already_bridged.len(), 2);
    assert_eq!(
        already_bridged[1].reasoning_mode,
        MessagesReasoningMode::Adaptive
    );
    assert_eq!(
        already_bridged[1].cache_write_ttl,
        CacheWriteTtl::FiveMinutes
    );
}

#[test]
fn runtime_preserves_an_explicit_messages_bridge_without_changing_its_policy() {
    let bindings = runtime_source_protocol_bindings(
        vec![
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                model_ids: vec!["claude-a".to_string(), "claude-b".to_string()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::OneHour,
                model_ids: vec!["claude-a".to_string()],
            },
        ],
        WireApi::Messages,
        &["claude-a".to_string(), "claude-b".to_string()],
    )
    .unwrap();

    assert_eq!(bindings.len(), 2);
    assert_eq!(bindings[1].model_ids, ["claude-a"]);
    assert_eq!(bindings[1].reasoning_mode, MessagesReasoningMode::Adaptive);
    assert_eq!(bindings[1].cache_write_ttl, CacheWriteTtl::OneHour);
}

#[test]
fn single_empty_protocol_binding_keeps_legacy_source_catalog() {
    let bindings = normalize_source_protocol_bindings(
        vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        }],
        WireApi::Responses,
        &["gpt-test".to_string()],
    )
    .unwrap();
    assert_eq!(bindings[0].model_ids, ["gpt-test"]);
}

#[test]
fn single_empty_gemini_bridge_stays_unconfirmed() {
    let bindings = normalize_source_protocol_bindings(
        vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToGemini,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: Vec::new(),
        }],
        WireApi::Responses,
        &["gemini-3-pro".to_string()],
    )
    .unwrap();

    assert!(bindings[0].model_ids.is_empty());
    assert_eq!(bindings[0].reasoning_mode, MessagesReasoningMode::Adaptive);
}

#[test]
fn bridge_binding_is_responses_only_and_reasoning_is_bridge_only() {
    let bridged = normalize_source_protocol_bindings(
        vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-test".to_string()],
        }],
        WireApi::Responses,
        &["claude-test".to_string()],
    )
    .unwrap();
    assert_eq!(bridged[0].adapter, SourceAdapter::ResponsesToMessages);
    assert_eq!(bridged[0].reasoning_mode, MessagesReasoningMode::Adaptive);

    let invalid_protocol = normalize_source_protocol_bindings(
        vec![SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-test".to_string()],
        }],
        WireApi::Messages,
        &["claude-test".to_string()],
    )
    .unwrap_err();
    assert!(invalid_protocol
        .to_string()
        .contains("cannot serve this client protocol"));

    let legacy_reasoning = normalize_source_protocol_bindings(
        vec![SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Budget,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-test".to_string()],
        }],
        WireApi::Responses,
        &["gpt-test".to_string()],
    )
    .unwrap();
    assert_eq!(
        legacy_reasoning[0].reasoning_mode,
        MessagesReasoningMode::Disabled
    );
}

#[test]
fn responses_native_and_messages_bridge_are_distinct_source_routes() {
    let bindings = normalize_source_protocol_bindings(
        vec![
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-test".to_string()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-test".to_string()],
            },
        ],
        WireApi::Responses,
        &["gpt-test".to_string(), "claude-test".to_string()],
    )
    .unwrap();

    assert_eq!(bindings.len(), 2);
    assert_ne!(bindings[0].key(), bindings[1].key());
    assert_eq!(bindings[0].model_ids, ["gpt-test"]);
    assert_eq!(bindings[1].model_ids, ["claude-test"]);
}

#[test]
fn model_can_use_two_distinct_routes_for_the_same_client_protocol() {
    let routes = normalize_source_protocol_bindings(
        vec![
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["shared-model".to_string()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::ResponsesToMessages,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["shared-model".to_string()],
            },
        ],
        WireApi::Responses,
        &["shared-model".to_string()],
    )
    .unwrap();

    assert_eq!(routes.len(), 2);
    assert_ne!(routes[0].key(), routes[1].key());
    assert_eq!(routes[0].model_ids, routes[1].model_ids);
}

#[test]
fn source_validation_rejects_unsafe_urls_and_redacts_secrets() {
    let mut source = ProviderSource {
        id: "source-1".to_string(),
        name: "Example".to_string(),
        base_url: "ftp://example.test/v1".to_string(),
        api_key: "upstream-secret".to_string(),
        wire_api: WireApi::Responses,
        models: vec!["model-1".to_string()],
    };
    assert!(source.validate().is_err());

    source.base_url = "https://user:password@example.test/v1".to_string();
    assert!(source.validate().is_err());
    assert!(!format!("{source:?}").contains("password"));

    source.base_url = "http://example.test/v1".to_string();
    assert!(source.validate().is_err());
    source.base_url = "http://127.0.0.1:14998/v1".to_string();
    assert!(source.validate().is_ok());
    assert!(!format!("{source:?}").contains("upstream-secret"));
}

#[test]
fn source_protocol_helpers_preserve_canonical_models_and_loopback_rules() {
    let source_models = ["gpt-test".to_string(), "claude-test".to_string()];
    let bindings = [
        SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-test".to_string()],
        },
        SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::ResponsesToMessages,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-test".to_string()],
        },
    ];

    assert_eq!(WireApi::Responses.as_str(), "responses");
    assert_eq!(WireApi::ChatCompletions.as_str(), "chat_completions");
    assert_eq!(WireApi::Messages.as_str(), "messages");
    assert_eq!(WireApi::Gemini.as_str(), "gemini");
    assert_eq!(
        WireApi::from_storage_value("chatcompletions"),
        Some(WireApi::ChatCompletions)
    );
    assert_eq!(WireApi::from_storage_value("unknown"), None);
    assert_eq!(
        source_models_for_wire_api(
            &bindings,
            WireApi::Responses,
            &source_models,
            WireApi::Responses,
        )
        .unwrap(),
        ["gpt-test", "claude-test"]
    );
    assert_eq!(
        runtime_source_models_for_any_wire_api(&bindings, WireApi::Responses, &source_models,)
            .unwrap(),
        ["gpt-test", "claude-test"]
    );
    assert!(runtime_source_supports_wire_api(
        &bindings,
        WireApi::Responses,
        &source_models,
        WireApi::Responses,
    )
    .unwrap());
    assert!(
        runtime_source_supports_any_wire_api(&bindings, WireApi::Responses, &source_models,)
            .unwrap()
    );
    assert!(is_loopback_url(&Url::parse("http://localhost").unwrap()));
    assert!(is_loopback_url(&Url::parse("http://[::1]").unwrap()));
    assert!(!is_loopback_url(
        &Url::parse("https://example.test").unwrap()
    ));
}

#[test]
fn base_url_normalization_removes_copied_terminal_api_endpoints() {
    for (input, expected) in [
        (
            "https://api.example.test/v1",
            "https://api.example.test/v1/",
        ),
        (
            "https://api.example.test/v1/models",
            "https://api.example.test/v1/",
        ),
        (
            "https://api.example.test/v1/responses",
            "https://api.example.test/v1/",
        ),
        (
            "https://api.example.test/v1/chat/completions",
            "https://api.example.test/v1/",
        ),
    ] {
        assert_eq!(normalized_base_url(input).unwrap().as_str(), expected);
    }
}

#[test]
fn reasoning_effort_never_claims_support_from_a_disabled_bridge() {
    let native = SourceProtocolBinding::legacy(WireApi::Responses, &[]);
    let disabled_bridge = SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::ResponsesToMessages,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: Vec::new(),
    };
    let adaptive_bridge = SourceProtocolBinding {
        reasoning_mode: MessagesReasoningMode::Adaptive,
        ..disabled_bridge.clone()
    };

    assert!(native.supports_reasoning_effort("provider-defined"));
    assert!(!disabled_bridge.supports_reasoning_effort("high"));
    assert!(adaptive_bridge.supports_reasoning_effort("high"));
    assert!(!adaptive_bridge.supports_reasoning_effort("very_high"));
}

#[test]
fn source_self_route_matches_only_the_same_gateway_endpoint() {
    assert!(source_points_to_gateway(
        "http://localhost:14998/v1",
        "http://127.0.0.1:14998/v1"
    ));
    assert!(source_points_to_gateway(
        "https://relay.example.test/v1/",
        "https://relay.example.test/v1"
    ));
    assert!(!source_points_to_gateway(
        "http://127.0.0.1:14999/v1",
        "http://127.0.0.1:14998/v1"
    ));
    assert!(!source_points_to_gateway(
        "https://provider.example.test/v1",
        "https://relay.example.test/v1"
    ));
}

#[test]
fn provider_stats_use_numeric_micro_usd_and_reject_lookalike_hosts() {
    assert_eq!(
        source_stats_endpoint(
            SourceStatsProvider::Zenith,
            "https://api.zenithmarket.dev/v1"
        )
        .unwrap()
        .as_str(),
        "https://api.zenithmarket.dev/v1/zenith/key/stats"
    );
    assert_eq!(
        source_stats_endpoint(
            SourceStatsProvider::OpenRouter,
            "https://openrouter.ai/api/v1/"
        )
        .unwrap()
        .as_str(),
        "https://openrouter.ai/api/v1/key"
    );
    assert_eq!(
        source_stats_provider("https://api.zenithmarket.dev/v1"),
        SourceStatsProvider::Zenith
    );
    assert_eq!(
        source_stats_provider("https://openrouter.ai.evil.test/api/v1"),
        SourceStatsProvider::Unsupported
    );
    let stats = openrouter_stats(&serde_json::json!({
        "data": { "total_credits": 12.5, "total_usage": 2.25 }
    }))
    .unwrap();
    assert_eq!(
        serde_json::to_value(&stats).unwrap()["provider"],
        "openrouter"
    );
    assert_eq!(stats.balance_micro_usd, Some(10_250_000));
    assert_eq!(stats.spent_micro_usd, Some(2_250_000));
    let stats = zenith_stats(&serde_json::json!({
        "data": {
            "displayBalanceMicrousd": 1234567,
            "spentCents": 250,
            "requests": 7,
            "totalTokens": 99
        }
    }))
    .unwrap();
    assert_eq!(stats.balance_micro_usd, Some(1_234_567));
    assert_eq!(stats.spent_micro_usd, Some(2_500_000));
    assert_eq!(stats.requests, Some(7));
}

#[test]
fn source_runtime_keeps_policy_edits_and_rebuilds_catalog_evidence() {
    let config = SourceProtocolConfig::default();
    let changed = SourceProtocolConfig {
        revision: 1,
        ..config.clone()
    };
    let models = vec!["gpt-test".to_string()];
    let previous = SourceTransportIdentity {
        id: "source",
        base_url: "https://example.test/v1",
        secret_ref: "source:source",
        wire_api: WireApi::Responses,
        protocol_bindings: &[],
        protocol_config: &config,
        models: &models,
    };
    let same = previous;
    assert!(source_runtime_policy_compatible(&[previous], &[same],));
    let next = SourceTransportIdentity {
        protocol_config: &changed,
        ..previous
    };
    assert!(!source_runtime_policy_compatible(&[previous], &[next],));
}

#[test]
fn source_priorities_update_known_sources_and_reject_unknown_ids() {
    let mut sources = vec![("source-a".to_string(), 0), ("source-b".to_string(), 1)];
    apply_source_priorities(
        &mut sources,
        &BTreeMap::from([("source-b".into(), 4), ("source-a".into(), 2)]),
        |source| source.0.as_str(),
        |source, priority| source.1 = priority,
    )
    .unwrap();
    assert_eq!(sources, [("source-a".into(), 2), ("source-b".into(), 4)]);
    let error = apply_source_priorities(
        &mut sources,
        &BTreeMap::from([("missing".into(), 3)]),
        |source| source.0.as_str(),
        |source, priority| source.1 = priority,
    )
    .unwrap_err();
    assert_eq!(error, "missing");
    assert_eq!(sources[0].1, 2);
}
