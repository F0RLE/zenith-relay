use super::*;
use serde_json::json;

#[test]
fn catalog_models_are_routable_without_generation_probes() {
    let config = SourceProtocolConfig::automatic("https://example.test/v1");
    let models = vec!["claude-test".into(), "gpt-test".into()];
    let routes = config
        .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
        .unwrap();
    assert_eq!(routes.len(), 4);
    assert!(routes.iter().all(|route| route.model_ids == models));
    assert!(catalog_capabilities(
        &json!({"data":[{"id":"gpt-test","pricing":{"input":1}}]}),
        1
    )
    .is_empty());
}

#[test]
fn participant_catalog_only_supplies_endpoint_identity() {
    let mut row = json!({"id":"future-model", "supported_reasoning_efforts":["low","high"],
        "capabilities":{"reasoning":true,"tools":false}});
    assert!(catalog_capabilities(&json!({"data":[row.clone()]}), 42).is_empty());
    row["supported_endpoint_types"] = json!(["messages"]);
    let observations = catalog_capabilities(&json!({"data":[row]}), 42);
    assert!(observations
        .iter()
        .all(|entry| entry.features.is_empty() && entry.reasoning_efforts.is_empty()));
    assert!(observations
        .iter()
        .any(|entry| entry.upstream_wire_api == WireApi::Messages && entry.status.available()));
}

#[test]
fn legacy_configuration_uses_automatic_routes() {
    let config = SourceProtocolConfig::default();
    let routes = config
        .resolve(
            "https://example.test/v1",
            &["gpt-test".into()],
            &[],
            WireApi::Responses,
        )
        .unwrap();
    assert_eq!(routes.len(), WireApi::ALL.len());
    assert!(routes.iter().all(|route| route.model_ids == ["gpt-test"]));
}

#[test]
fn legacy_bridge_supplies_physical_protocol_without_restricting_clients() {
    let legacy = [SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::ResponsesToMessages,
        reasoning_mode: MessagesReasoningMode::Adaptive,
        cache_write_ttl: CacheWriteTtl::Provider,
        model_ids: vec![],
    }];
    for (url, upstream) in [
        ("https://example.test/v1", WireApi::Messages),
        (
            "https://example.test/v1/chat/completions",
            WireApi::ChatCompletions,
        ),
    ] {
        let routes = SourceProtocolConfig::automatic(url)
            .resolve(url, &["future-model".into()], &legacy, WireApi::Responses)
            .unwrap();
        assert_eq!(routes.len(), WireApi::ALL.len());
        assert!(routes.iter().all(|route| {
            route.model_ids == ["future-model"]
                && route.adapter.upstream_protocol(route.wire_api).wire_api() == upstream
        }));
    }
}

#[test]
fn endpoint_metadata_and_gemini_methods_are_scoped_declarations() {
    let observations = catalog_capabilities(
        &json!({"data":[
            {"id":"mixed","supported_endpoint_types":["openai","anthropic"]},
            {"id":"unknown"}
        ]}),
        42,
    );
    assert_eq!(observations.len(), 4);
    assert!(observations.iter().all(|entry| entry.model_id == "mixed"));
    assert_eq!(
        observations
            .iter()
            .filter(|entry| entry.status.available())
            .count(),
        2
    );
    let gemini = catalog_capabilities(
        &json!({"models":[
            {"name":"models/test","supportedGenerationMethods":["generateContent","countTokens"]},
            {"name":"models/embed","supportedGenerationMethods":["embedContent"]}
        ]}),
        43,
    );
    assert_eq!(gemini.len(), 2);
    assert_eq!(gemini[0].upstream_wire_api, WireApi::Gemini);
    assert!(!gemini[0]
        .features
        .contains_key(&ProtocolFeature::FunctionTools));
    let mut config =
        SourceProtocolConfig::automatic("https://generativelanguage.googleapis.com/v1beta");
    config.merge_catalog(gemini);
    let routes = config
        .resolve(
            "https://generativelanguage.googleapis.com/v1beta",
            &["test".into(), "embed".into()],
            &[],
            WireApi::Gemini,
        )
        .unwrap();
    assert_eq!(routes.len(), 4);
    assert!(routes
        .iter()
        .all(|route| route.model_ids == ["test", "embed"]));
}

#[test]
fn each_client_protocol_prefers_its_native_upstream() {
    let models = vec!["mixed".into()];
    let mut config = SourceProtocolConfig::default();
    config.merge_catalog(catalog_capabilities(
        &json!({"data":[{
            "id":"mixed",
            "supported_endpoint_types":["responses", "messages"]
        }]}),
        42,
    ));

    let routes = config
        .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
        .unwrap();
    let responses = routes
        .iter()
        .find(|route| route.wire_api == WireApi::Responses)
        .unwrap();
    let messages = routes
        .iter()
        .find(|route| route.wire_api == WireApi::Messages)
        .unwrap();

    assert_eq!(responses.adapter, SourceAdapter::Native);
    assert_eq!(messages.adapter, SourceAdapter::Native);
}

#[test]
fn failed_generation_probe_does_not_remove_catalog_models() {
    let models = vec!["test".into()];
    let mut config = SourceProtocolConfig {
        capabilities: catalog_capabilities(
            &json!({"data":[{"id":"test","supported_endpoint_types":["responses"]}]}),
            1,
        ),
        ..Default::default()
    };
    config.capabilities.push(ModelEndpointCapability {
        model_id: "test".into(),
        upstream_wire_api: WireApi::Responses,
        status: CapabilityStatus::Confirmed,
        origin: CapabilityOrigin::GenerationProbe,
        checked_at_ms: 2,
        features: BTreeMap::from([(ProtocolFeature::Text, CapabilityStatus::Unsupported)]),
        reasoning_efforts: vec![],
    });
    let resolved = config
        .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
        .unwrap();
    assert_eq!(resolved.len(), 4);
    assert!(resolved.iter().all(|route| route.model_ids == ["test"]));
    assert!(resolved
        .iter()
        .any(|route| route.wire_api == WireApi::Responses));
    let capabilities = config.effective_capabilities("https://example.test/v1", &models);
    let responses = capabilities
        .iter()
        .find(|entry| entry.upstream_wire_api == WireApi::Responses)
        .unwrap();
    assert_eq!(responses.origin, CapabilityOrigin::Catalog);
    assert_eq!(responses.status, CapabilityStatus::Declared);
    assert_ne!(
        responses.features.get(&ProtocolFeature::Text),
        Some(&CapabilityStatus::Unsupported)
    );
}

#[test]
fn successful_generation_probe_does_not_override_catalog_routes() {
    let models = vec!["test".into()];
    let mut config = SourceProtocolConfig {
        capabilities: catalog_capabilities(
            &json!({"data":[{"id":"test","supported_endpoint_types":["responses"]}]}),
            1,
        ),
        ..Default::default()
    };
    config.capabilities.push(ModelEndpointCapability {
        model_id: "test".into(),
        upstream_wire_api: WireApi::ChatCompletions,
        status: CapabilityStatus::Confirmed,
        origin: CapabilityOrigin::GenerationProbe,
        checked_at_ms: 2,
        features: BTreeMap::from([(ProtocolFeature::Text, CapabilityStatus::Confirmed)]),
        reasoning_efforts: vec![],
    });

    let routes = config
        .resolve("https://example.test/v1", &models, &[], WireApi::Responses)
        .unwrap();
    assert!(routes.iter().all(|route| {
        route.adapter.upstream_protocol(route.wire_api).wire_api() == WireApi::Responses
    }));
    assert!(config
        .effective_capabilities("https://example.test/v1", &models)
        .iter()
        .all(|capability| capability.origin != CapabilityOrigin::GenerationProbe));
}

#[test]
fn stored_participant_features_do_not_override_endpoint_identity() {
    let config = SourceProtocolConfig {
        capabilities: vec![ModelEndpointCapability {
            model_id: "synthetic-model".into(),
            upstream_wire_api: WireApi::Messages,
            status: CapabilityStatus::Declared,
            origin: CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: BTreeMap::from([(ProtocolFeature::Text, CapabilityStatus::Unsupported)]),
            reasoning_efforts: vec![],
        }],
        ..Default::default()
    };
    let routes = config
        .resolve(
            "https://example.test/v1",
            &["synthetic-model".into()],
            &[],
            WireApi::Responses,
        )
        .unwrap();
    assert_eq!(routes.len(), 4);
    assert!(routes.iter().all(
        |route| route.adapter.upstream_protocol(route.wire_api).wire_api() == WireApi::Messages
    ));
}

#[test]
fn old_configuration_ignores_legacy_mode_and_checks_probe_revision() {
    let mut config: SourceProtocolConfig = serde_json::from_value(json!({})).unwrap();
    let legacy: SourceProtocolConfig = serde_json::from_value(json!({"mode":"manual"})).unwrap();
    assert_eq!(legacy, SourceProtocolConfig::default());
    let observation = ModelEndpointCapability {
        model_id: "test".into(),
        upstream_wire_api: WireApi::Responses,
        status: CapabilityStatus::Confirmed,
        origin: CapabilityOrigin::GenerationProbe,
        checked_at_ms: 42,
        features: BTreeMap::new(),
        reasoning_efforts: vec![],
    };
    config.invalidate("https://example.test/v1");
    assert!(!config.apply_probe(0, observation.clone()));
    assert!(config.apply_probe(1, observation));
    config.invalidate("https://other.test/v1");
    assert!(config.capabilities.is_empty());
}

#[test]
fn explicit_endpoint_and_service_profile_do_not_match_lookalikes() {
    assert_eq!(
        endpoint_url_protocol("https://example.test/custom/chat/completions"),
        Some(WireApi::ChatCompletions)
    );
    assert_eq!(
        service_protocol("https://openrouter.ai/api/v1"),
        Some(WireApi::ChatCompletions)
    );
    assert_eq!(
        service_protocol("https://openrouter.ai.example.test/v1"),
        None
    );
    assert_eq!(
        service_protocol("https://api.zenithmarket.dev/v1"),
        Some(WireApi::Responses)
    );
    assert_eq!(
        service_protocol("https://api.zenithmarket.dev.example.test/v1"),
        None
    );
}

#[test]
fn known_responses_service_keeps_unprobed_catalog_models_routable() {
    let models = vec![
        "gpt-5.6-sol".into(),
        "claude-fable-5".into(),
        "gemini-3.8-flash".into(),
        "grok-4.6".into(),
    ];
    let mut config = SourceProtocolConfig::automatic("https://api.zenithmarket.dev/v1");
    config.capabilities.push(ModelEndpointCapability {
        model_id: "gpt-5.6-sol".into(),
        upstream_wire_api: WireApi::Responses,
        status: CapabilityStatus::Confirmed,
        origin: CapabilityOrigin::GenerationProbe,
        checked_at_ms: 42,
        features: BTreeMap::from([(ProtocolFeature::Text, CapabilityStatus::Confirmed)]),
        reasoning_efforts: vec![],
    });

    let routes = config
        .resolve(
            "https://api.zenithmarket.dev/v1",
            &models,
            &[],
            WireApi::Responses,
        )
        .unwrap();

    assert_eq!(routes.len(), WireApi::ALL.len());
    for client in WireApi::ALL {
        let route = routes
            .iter()
            .find(|route| route.wire_api == client)
            .expect("every client protocol should have an adapter route");
        assert_eq!(route.model_ids, models);
        assert_eq!(
            route.adapter.upstream_protocol(client).wire_api(),
            WireApi::Responses
        );
    }
}
