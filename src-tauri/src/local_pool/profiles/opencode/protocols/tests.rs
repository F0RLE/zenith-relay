use super::*;
use zenith_relay_core::protocol::ModelProtocolRoute;

mod pooled_capabilities;

#[test]
fn sdk_selection_and_refresh_omit_nonstreaming_and_interactions_only_models() {
    let mut buffered = model("buffered-only", WireApi::Responses);
    for route in &mut buffered.protocol_routes {
        route.features.insert(
            zenith_relay_core::ProtocolFeature::Streaming,
            zenith_relay_core::CapabilityStatus::Unsupported,
        );
    }
    let interactions = model("interactions-only", WireApi::GeminiInteractions);
    let supported = model("supported", WireApi::Messages);
    let models = [buffered, interactions, supported];
    let selected = super::super::model_ids(&models);
    assert_eq!(
        selected
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["supported"]
    );
    let mut config = Map::new();
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &models,
    )
    .unwrap();
    for (_, id, _) in GROUPS {
        let Some(provider) = config["provider"].get(id) else {
            continue;
        };
        let configured = provider["models"].as_object().unwrap();
        assert!(!configured.contains_key("buffered-only"));
        assert!(!configured.contains_key("interactions-only"));
    }
    assert!(
        config["provider"]["zenith-relay-messages"]["models"]
            .get("supported")
            .is_some()
    );
}

#[test]
fn direct_connection_chooses_a_streaming_native_protocol_and_rejects_an_empty_selection() {
    use zenith_relay_core::SourceProtocolBinding;
    let ids = ["opaque-model".to_owned()];
    let mut source = super::super::tests::source(vec![
        SourceProtocolBinding::legacy(WireApi::Responses, &ids),
        SourceProtocolBinding::legacy(WireApi::Messages, &ids),
    ]);
    source.models = ids.to_vec();
    let metadata =
        ModelMetadataCatalog::from_models_dev_json(r#"{"test/opaque-model":{}}"#).unwrap();
    source.protocol_config.capabilities = [
        (WireApi::Responses, "unsupported"),
        (WireApi::Messages, "declared"),
    ]
    .map(|(protocol, status)| {
        serde_json::from_value(json!({
            "modelId":"opaque-model","upstreamWireApi":protocol,"status":"declared",
            "origin":"catalog","checkedAtMs":1,"features":{"streaming":status}
        }))
        .unwrap()
    })
    .to_vec();
    let mut config = Map::new();
    apply_source(&mut config, &source, "synthetic", &metadata, true).unwrap();
    assert!(
        config["provider"][PROVIDER_ID]["models"]
            .get("opaque-model")
            .is_none()
    );
    assert!(
        config["provider"]["zenith-relay-messages"]["models"]
            .get("opaque-model")
            .is_some()
    );
    for endpoint in &mut source.protocol_config.capabilities {
        endpoint.features.insert(
            zenith_relay_core::ProtocolFeature::Streaming,
            zenith_relay_core::CapabilityStatus::Unsupported,
        );
    }
    let original = config.clone();
    assert!(apply_source(&mut config, &source, "synthetic", &metadata, true).is_err());
    assert_eq!(config, original);
}

fn model(id: &str, native: WireApi) -> ModelSummary {
    let mut model = super::super::tests::model(id, true);
    model.protocol_routes = WireApi::ALL
        .into_iter()
        .map(|client| ModelProtocolRoute {
            client_wire_api: client,
            upstream_wire_api: native,
            features: BTreeMap::new(),
            reasoning_efforts: vec!["high".into()],
            token_limits: Default::default(),
        })
        .collect();
    model
}

#[test]
fn direct_profile_variants_follow_exact_endpoint_denials_and_effort_intersections() {
    use zenith_relay_core::{ModelEndpointCapability, SourceProtocolBinding};

    let metadata = ModelMetadataCatalog::from_models_dev_json(r#"{
        "test/opaque-model":{"reasoning":true,"reasoning_effort_levels":["low","medium","high"],"tool_call":true},
        "test/other-model":{"reasoning":true,"reasoning_effort_levels":["low","high"],"tool_call":true}
    }"#).unwrap();
    let ids = vec!["opaque-model".into(), "other-model".into()];
    for protocol in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
        let mut source =
            super::super::tests::source(vec![SourceProtocolBinding::legacy(protocol, &ids)]);
        source.models = ids.clone();
        source.protocol_config.endpoint_hint = Some(protocol);
        let endpoint = |features, efforts| -> ModelEndpointCapability {
            serde_json::from_value(json!({
                "modelId":"opaque-model","upstreamWireApi":protocol,"status":"declared",
                "origin":"catalog","checkedAtMs":1,"features":features,"reasoningEfforts":efforts
            }))
            .unwrap()
        };
        let provider_id = GROUPS
            .iter()
            .find(|(wire, _, _)| *wire == protocol)
            .unwrap()
            .1;
        source.protocol_config.capabilities = vec![endpoint(
            json!({"function_tools":"unsupported"}),
            json!(["high", "max"]),
        )];
        let mut config = Map::new();
        apply_source(&mut config, &source, "synthetic", &metadata, false).unwrap();
        let models = &config["provider"][provider_id]["models"];
        assert_eq!(models["opaque-model"]["tool_call"], false);
        assert_eq!(models["opaque-model"]["reasoning"], true);
        assert_eq!(models["opaque-model"]["variants"]["low"]["disabled"], true);
        assert!(
            models["opaque-model"]["variants"]["high"]
                .get("disabled")
                .is_none()
        );
        assert_eq!(models["other-model"]["tool_call"], true);
        assert!(
            models["other-model"]["variants"]["low"]
                .get("disabled")
                .is_none()
        );

        source.protocol_config.capabilities = vec![endpoint(
            json!({"reasoning":"unsupported"}),
            json!(["high"]),
        )];
        apply_source(&mut config, &source, "synthetic", &metadata, false).unwrap();
        let model = &config["provider"][provider_id]["models"]["opaque-model"];
        assert_eq!(model["tool_call"], true);
        assert_eq!(model["reasoning"], false);
        assert_eq!(model["variants"]["high"]["disabled"], true);
        assert_eq!(
            metadata.capabilities_for("opaque-model").reasoning,
            Some(true)
        );
    }
}

#[test]
fn direct_profiles_keep_numeric_limits_scoped_to_the_selected_source_endpoint() {
    use zenith_relay_core::{ModelEndpointCapability, SourceProtocolBinding};

    let metadata = ModelMetadataCatalog::from_models_dev_json(
        r#"{
        "test/opaque-model":{
            "reasoning":true,"reasoning_effort_levels":["low","high"],"tool_call":true,
            "limit":{"context":128000,"input":96000,"output":32000}
        },
        "test/other-model":{"limit":{"context":64000,"input":48000,"output":16000}}
    }"#,
    )
    .unwrap();
    for protocol in [WireApi::Responses, WireApi::Messages] {
        let ids = vec!["opaque-model".into(), "other-model".into()];
        let mut source =
            super::super::tests::source(vec![SourceProtocolBinding::legacy(protocol, &ids)]);
        source.models = ids;
        source.wire_api = protocol;
        source.protocol_config.endpoint_hint = Some(protocol);
        let capability = |upstream, limits| -> ModelEndpointCapability {
            serde_json::from_value(json!({
                "modelId":"opaque-model","upstreamWireApi":upstream,
                "status":"unknown","origin":"catalog","checkedAtMs":1,
                "tokenLimits":limits
            }))
            .unwrap()
        };
        source.protocol_config.capabilities = vec![
            capability(
                protocol,
                json!({"context":32000,"input":24000,"output":8000}),
            ),
            capability(
                if protocol == WireApi::Responses {
                    WireApi::Messages
                } else {
                    WireApi::Responses
                },
                json!({"context":1000,"input":800,"output":200}),
            ),
        ];
        let provider_id = GROUPS
            .iter()
            .find(|(wire, _, _)| *wire == protocol)
            .unwrap()
            .1;
        let mut config = Map::new();
        apply_source(&mut config, &source, "synthetic", &metadata, false).unwrap();
        let models = &config["provider"][provider_id]["models"];
        let restricted = json!({"context":32000,"input":24000,"output":8000});
        let reference = json!({"context":128000,"input":96000,"output":32000});
        assert_eq!(models["opaque-model"]["limit"], restricted);
        assert_eq!(
            models["other-model"]["limit"],
            json!({"context":64000,"input":48000,"output":16000})
        );
        assert_eq!(models["opaque-model"]["reasoning"], true);
        assert_eq!(models["opaque-model"]["tool_call"], true);
        assert_eq!(
            metadata.capabilities_for("opaque-model").context_limit,
            Some(128000)
        );

        source.protocol_config.capabilities = vec![capability(
            protocol,
            json!({"context":256000,"input":192000,"output":64000}),
        )];
        apply_source(&mut config, &source, "synthetic", &metadata, false).unwrap();
        // Existing smaller windows are user-editable preferences, not proof
        // that a fresh profile must retain the old source ceiling.
        assert_eq!(
            config["provider"][provider_id]["models"]["opaque-model"]["limit"],
            restricted
        );
        let mut fresh = Map::new();
        apply_source(&mut fresh, &source, "synthetic", &metadata, false).unwrap();
        assert_eq!(
            fresh["provider"][provider_id]["models"]["opaque-model"]["limit"],
            reference
        );

        source.protocol_config.capabilities.clear();
        fresh.clear();
        apply_source(&mut fresh, &source, "synthetic", &metadata, false).unwrap();
        assert_eq!(
            fresh["provider"][provider_id]["models"]["opaque-model"]["limit"],
            reference
        );
    }
}

#[test]
fn direct_interactions_sources_require_an_sdk_supported_native_route() {
    use zenith_relay_core::SourceProtocolBinding;

    let mut source = super::super::tests::source(vec![
        SourceProtocolBinding::legacy(WireApi::GeminiInteractions, &["interactions-only".into()]),
        SourceProtocolBinding::legacy(WireApi::Responses, &["supported".into()]),
    ]);
    source.models = vec!["interactions-only".into(), "supported".into()];
    assert_eq!(
        super::super::source_opencode_models(&source).unwrap(),
        vec!["supported"]
    );
    let mut config = Map::new();
    let metadata = ModelMetadataCatalog::empty();
    apply_source(&mut config, &source, "synthetic", &metadata, false).unwrap();
    assert!(
        config["provider"][PROVIDER_ID]["models"]
            .get("supported")
            .is_some()
    );
    for (_, provider_id, _) in GROUPS {
        assert!(
            config["provider"][provider_id]["models"]
                .get("interactions-only")
                .is_none()
        );
    }

    source.models = vec!["interactions-only".into()];
    source.protocol_bindings.truncate(1);
    config.insert("model".into(), format!("{PROVIDER_ID}/supported").into());
    assert!(super::super::source_opencode_models(&source).is_err());
    apply_source(&mut config, &source, "synthetic", &metadata, false).unwrap();
    assert!(!config.contains_key("model"));
    for (_, provider_id, _) in GROUPS {
        assert!(config["provider"].get(provider_id).is_none_or(|provider| {
            provider["models"]
                .as_object()
                .is_some_and(|models| models.is_empty())
        }));
    }
}

#[test]
fn interactions_only_models_are_not_advertised_to_an_unsupported_sdk() {
    let mut native = model("native-interactions", WireApi::GeminiInteractions);
    native.protocol_routes = vec![ModelProtocolRoute {
        client_wire_api: WireApi::GeminiInteractions,
        upstream_wire_api: WireApi::GeminiInteractions,
        features: BTreeMap::new(),
        reasoning_efforts: Vec::new(),
        token_limits: Default::default(),
    }];
    let supported = model("supported", WireApi::Gemini);
    let selected = super::super::models::model_ids(&[native, supported]);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, "supported");
}

#[test]
fn groups_new_models_by_native_sdk_and_preserves_existing_selection_and_options() {
    let models = [
        model("open", WireApi::Responses),
        model("chat", WireApi::ChatCompletions),
        model("claude", WireApi::Messages),
        model("gemini", WireApi::Gemini),
    ];
    let mut config = Map::new();
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &models,
    )
    .unwrap();
    for (_, id, npm) in GROUPS {
        assert_eq!(config["provider"][id]["npm"], npm);
        assert_eq!(
            config["provider"][id]["models"].as_object().unwrap().len(),
            1
        );
    }
    assert_eq!(
        config["provider"]["zenith-relay-gemini"]["options"]["baseURL"],
        "http://127.0.0.1:14998/v1beta"
    );
    // A previous Relay version pinned the native Messages model to the
    // Responses SDK. It must move to its native group and keep user edits.
    let providers = config.get_mut("provider").unwrap();
    providers["zenith-relay-messages"]["models"]
        .as_object_mut()
        .unwrap()
        .remove("claude");
    providers["zenith-relay"]["models"]["claude"] =
        json!({"name":"My model","options":{"custom":true}});
    providers["zenith-relay"]["options"]["timeout"] = 45000.into();
    config.insert("model".into(), "zenith-relay/claude".into());
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &models,
    )
    .unwrap();
    assert_eq!(config["model"], "zenith-relay-messages/claude");
    assert!(
        config["provider"]["zenith-relay"]["models"]
            .get("claude")
            .is_none()
    );
    let moved = &config["provider"]["zenith-relay-messages"]["models"]["claude"];
    assert_eq!(moved["name"], "My model");
    assert_eq!(moved["options"]["custom"], true);
    assert_eq!(
        config["provider"]["zenith-relay"]["options"]["timeout"],
        45000
    );
}

#[test]
fn converted_only_models_keep_their_existing_provider() {
    let mut bridged = model("bridged", WireApi::Messages);
    bridged
        .protocol_routes
        .retain(|route| route.client_wire_api != WireApi::Messages);
    let mut config = Map::from_iter([(
        "provider".into(),
        json!({"zenith-relay-chat":{"models":{"bridged":{}}}}),
    )]);
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &[bridged],
    )
    .unwrap();
    assert!(
        config["provider"]["zenith-relay-chat"]["models"]
            .get("bridged")
            .is_some()
    );
    assert!(
        config["provider"]["zenith-relay"]["models"]
            .get("bridged")
            .is_none()
    );
}

#[test]
fn sdk_reasoning_denial_cannot_retain_or_reintroduce_generated_efforts() {
    for protocol in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
        for mut configured in [
            json!({"reasoning":false}),
            json!({"reasoning":false,"variants":{"high":{"reasoningEffort":"high"}}}),
        ] {
            set_variants(&mut configured, protocol);
            for effort in [
                "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
            ] {
                assert_eq!(configured["variants"][effort], json!({"disabled":true}));
            }
            let stable = configured.clone();
            set_variants(&mut configured, protocol);
            assert_eq!(configured, stable);
        }
    }
}

#[test]
fn sdk_defaults_cannot_reintroduce_unsupported_reasoning_variants() {
    for protocol in WireApi::ALL {
        let mut configured = json!({"reasoning":true,"variants":{"high":{}}});
        set_variants(&mut configured, protocol);
        assert_eq!(
            configured["variants"]["medium"]["disabled"], true,
            "{protocol:?}"
        );
        assert!(configured["variants"]["high"].get("disabled").is_none());
        let snapshot = configured.clone();
        set_variants(&mut configured, protocol);
        assert_eq!(configured, snapshot);
        let previous = json!({"models":{"model":configured}});
        let mut refreshed = json!({"reasoning":true,"variants":{"medium":{}}});
        set_variants(&mut refreshed, protocol);
        let merged = merge_provider(Some(&previous), json!({"models":{"model":refreshed}}));
        let variants = &merged["models"]["model"]["variants"];
        assert!(variants["medium"].get("disabled").is_none(), "{protocol:?}");
        assert_eq!(variants["high"]["disabled"], true, "{protocol:?}");
        let mut default_only = json!({"reasoning":true});
        set_variants(&mut default_only, protocol);
        assert_eq!(default_only["variants"]["medium"]["disabled"], true);
    }
}

#[test]
fn sdk_variants_disable_thinking_without_forwarding_invalid_effort_names() {
    for (protocol, mut expected) in [
        (
            WireApi::Messages,
            json!({"none":{"thinking":{"type":"disabled"}},"high":{"thinking":{"type":"adaptive"},"effort":"high"},"max":{"thinking":{"type":"adaptive"},"effort":"max"}}),
        ),
        (
            WireApi::Gemini,
            json!({"none":{"thinkingConfig":{"thinkingBudget":0}},"minimal":{"thinkingConfig":{"thinkingLevel":"minimal"}},"high":{"thinkingConfig":{"thinkingLevel":"high"}}}),
        ),
    ] {
        for effort in [
            "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
        ] {
            if expected.get(effort).is_none() {
                expected[effort] = json!({"disabled":true});
            }
        }
        let mut value = json!({"variants":{"none":{},"minimal":{},"high":{},"max":{},"ultra":{}}});
        set_variants(&mut value, protocol);
        assert_eq!(value["variants"], expected);
        let previous = json!({"models":{"model":{"variants":{"none":{"thinking":{"type":"adaptive"},"effort":"none"}}}}});
        let merged = merge_provider(Some(&previous), json!({"models":{"model":value}}));
        assert_eq!(merged["models"]["model"]["variants"], expected);
    }
}

#[test]
fn direct_refresh_preserves_selection_and_narrows_reference_capabilities() {
    use zenith_relay_core::{
        CapabilityOrigin, CapabilityStatus, ModelEndpointCapability, ProtocolFeature,
        SourceProtocolBinding,
    };
    let ids = vec!["claude".into(), "gpt-test".into()];
    let mut source = super::super::tests::source(vec![
        SourceProtocolBinding::legacy(WireApi::Responses, &ids),
        SourceProtocolBinding::legacy(WireApi::Messages, &ids),
    ]);
    source
        .protocol_config
        .capabilities
        .push(ModelEndpointCapability {
            model_id: "claude".into(),
            upstream_wire_api: WireApi::Messages,
            status: CapabilityStatus::Declared,
            origin: CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: BTreeMap::from([(
                ProtocolFeature::FunctionTools,
                CapabilityStatus::Unsupported,
            )]),
            reasoning_efforts: vec!["none".into(), "high".into()],
            token_limits: Default::default(),
            service_tiers: None,
        });
    let metadata = ModelMetadataCatalog::from_models_dev_json(r#"{
        "test/claude":{"reasoning":true,"reasoning_effort_levels":["none","low","high","ultra"],"tool_call":true}
    }"#).unwrap();
    let mut config = Map::from_iter([
        ("model".into(), "user-provider/selected".into()),
        (
            "provider".into(),
            json!({"zenith-relay-messages":{"models":{"claude":{},"gpt-test":{}}}}),
        ),
    ]);
    apply_source(&mut config, &source, "synthetic", &metadata, false).unwrap();
    assert_eq!(config["model"], "user-provider/selected");
    let models = &config["provider"]["zenith-relay-messages"]["models"];
    // Refresh keeps the user's selection, while moving GPT to its native SDK.
    assert!(models.get("gpt-test").is_none());
    assert_eq!(models["claude"]["tool_call"], false);
    assert_eq!(
        models["claude"]["variants"],
        json!({
            "none":{"thinking":{"type":"disabled"}},
            "minimal":{"disabled":true},
            "low":{"disabled":true},
            "medium":{"disabled":true},
            "high":{"thinking":{"type":"adaptive"},"effort":"high"},
            "xhigh":{"disabled":true},
            "max":{"disabled":true},
            "ultra":{"disabled":true}
        })
    );
    let fallback_models = config["provider"]["zenith-relay"]["models"]
        .as_object()
        .unwrap();
    assert_eq!(fallback_models.len(), 3);
    assert!(fallback_models.contains_key("gpt-test"));
    assert!(fallback_models.contains_key("gpt-other"));
    assert!(fallback_models.contains_key("chat-only"));
}

#[test]
fn unavailable_models_and_obsolete_generated_reasoning_variants_are_removed() {
    let mut supported = model("claude", WireApi::Messages);
    supported.catalog_provider = Some("anthropic".into());
    supported.catalog_reasoning = Some(true);
    supported.catalog_reasoning_effort_levels = vec!["high".into(), "ultra".into()];
    let mut unknown = model("unknown", WireApi::Responses);
    unknown.protocol_routes.clear();
    let mut config = Map::from_iter([(
        "provider".into(),
        json!({"zenith-relay-messages":{"models":{"claude":{"variants":{"ultra":{"effort":"ultra"},"custom":{"temperature":0.2},"deliberate":{"thinking":{"type":"enabled","budgetTokens":4096}}}}}}}),
    )]);
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        &[supported, unknown],
    )
    .unwrap();
    let models = &config["provider"]["zenith-relay-messages"]["models"];
    assert_eq!(models["claude"]["variants"]["high"]["effort"], "high");
    assert_eq!(models["claude"]["variants"]["ultra"]["disabled"], true);
    assert_eq!(models["claude"]["variants"]["custom"]["temperature"], 0.2);
    assert_eq!(
        models["claude"]["variants"]["deliberate"]["thinking"]["budgetTokens"],
        4096
    );
    assert!(!config["provider"].to_string().contains("unknown"));
}

#[test]
fn native_model_migration_tolerates_malformed_managed_provider_fields() {
    for target_provider in [
        json!("invalid"),
        json!({"models": "invalid", "name": "Custom"}),
    ] {
        let mut config = Map::from_iter([
            ("model".into(), "zenith-relay/claude".into()),
            (
                "provider".into(),
                json!({
                    "zenith-relay": {"models": {"claude": {"name": "My Claude", "options": {"custom": true}}}},
                    "zenith-relay-messages": target_provider.clone(),
                    "user-provider": {"models": {"user-model": {"name": "Untouched"}}}
                }),
            ),
        ]);
        apply(
            &mut config,
            "http://127.0.0.1:14998/v1",
            "synthetic",
            &[model("claude", WireApi::Messages)],
        )
        .unwrap();
        assert_eq!(config["model"], "zenith-relay-messages/claude");
        let migrated = &config["provider"]["zenith-relay-messages"]["models"]["claude"];
        assert_eq!(migrated["name"], "My Claude");
        assert_eq!(migrated["options"]["custom"], true);
        assert_eq!(
            config["provider"]["user-provider"]["models"]["user-model"]["name"],
            "Untouched"
        );
        if target_provider.is_object() {
            assert_eq!(
                config["provider"]["zenith-relay-messages"]["name"],
                "Custom"
            );
        }
    }
}
