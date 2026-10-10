use super::*;
use zenith_relay_core::protocol::ModelProtocolRoute;

fn model(id: &str, native: WireApi) -> ModelSummary {
    let mut model = super::super::tests::model(id, true);
    model.protocol_routes = WireApi::ALL
        .into_iter()
        .map(|client| ModelProtocolRoute {
            client_wire_api: client,
            upstream_wire_api: native,
            features: BTreeMap::new(),
            reasoning_efforts: vec!["high".into()],
        })
        .collect();
    model
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
    assert!(config["provider"]["zenith-relay"]["models"]
        .get("claude")
        .is_none());
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
    assert!(config["provider"]["zenith-relay-chat"]["models"]
        .get("bridged")
        .is_some());
    assert!(config["provider"]["zenith-relay"]["models"]
        .get("bridged")
        .is_none());
}

#[test]
fn sdk_variants_disable_thinking_without_forwarding_invalid_effort_names() {
    for (protocol, expected) in [
        (
            WireApi::Messages,
            json!({"none":{"thinking":{"type":"disabled"}},"high":{"thinking":{"type":"adaptive"},"effort":"high"},"max":{"thinking":{"type":"adaptive"},"effort":"max"}}),
        ),
        (
            WireApi::Gemini,
            json!({"none":{"thinkingConfig":{"thinkingBudget":0}},"minimal":{"thinkingConfig":{"thinkingLevel":"minimal"}},"high":{"thinkingConfig":{"thinkingLevel":"high"}}}),
        ),
    ] {
        let mut value = json!({"variants":{"none":{},"minimal":{},"high":{},"max":{},"ultra":{}}});
        set_variants(&mut value, protocol);
        assert_eq!(value["variants"], expected);
        let previous = json!({"models":{"model":{"variants":{"none":{"thinking":{"type":"adaptive"},"effort":"none"}}}}});
        let merged = merge_provider(Some(&previous), json!({"models":{"model":value}}));
        assert_eq!(merged["models"]["model"]["variants"], expected);
    }
}

#[test]
fn direct_refresh_preserves_selection_and_uses_reference_capabilities() {
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
    assert_eq!(models["claude"]["tool_call"], true);
    assert_eq!(
        models["claude"]["variants"],
        json!({
            "none":{"thinking":{"type":"disabled"}},
            "low":{"thinking":{"type":"adaptive"},"effort":"low"},
            "high":{"thinking":{"type":"adaptive"},"effort":"high"}
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
    assert!(models["claude"]["variants"].get("ultra").is_none());
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
