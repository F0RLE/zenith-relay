use super::*;

fn source_record() -> ProviderSourceRecord {
    ProviderSourceRecord {
        id: "source".into(),
        name: "Provider".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "https://provider.test/v1".into(),
        secret_ref: "source:test".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["model-a".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        last_used_at: None,
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    }
}

#[test]
fn messages_wire_api_is_accepted_at_the_desktop_boundary() {
    let mut source = source_record();
    source.wire_api = WireApi::Messages;
    source.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Messages,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: source.models.clone(),
    }];

    source.normalize();
    source.validate_protocol_bindings().unwrap();
    assert!(source.validate_protocol_bindings().is_ok());
}

#[test]
fn source_wide_catalog_binding_is_preserved_when_validated() {
    let mut source = source_record();
    source.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: Vec::new(),
    }];

    source.validate_protocol_bindings().unwrap();

    assert!(source.protocol_bindings[0].model_ids.is_empty());
    assert_eq!(
        source.effective_protocol_bindings().unwrap()[0].model_ids,
        ["model-a"]
    );
}

#[test]
fn failed_automatic_discovery_keeps_the_source_without_advertising_models() {
    let source = ProviderSource {
        id: "source".into(),
        name: "Provider".into(),
        base_url: "https://provider.test/v1".into(),
        api_key: "secret".into(),
        wire_api: WireApi::Responses,
        models: Vec::new(),
    };
    let configured = vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["stale-model".into()],
    }];

    let discovery = empty_source_discovery(&source, &configured).unwrap();

    assert!(discovery.models.is_empty());
    assert_eq!(discovery.protocol_bindings.len(), 1);
    assert!(discovery.protocol_bindings[0].model_ids.is_empty());
}

#[test]
fn detected_prices_follow_the_source_upstream() {
    let mut source = source_record();
    source.detected_model_prices.insert(
        "model-a".into(),
        ApiModelPriceOverride {
            input_micro_usd_per_million: 1_000_000,
            cached_input_micro_usd_per_million: None,
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: 2_000_000,
        },
    );
    let base_url = source.base_url.clone();

    assert_eq!(
        detected_prices_for_upstream(&source, &base_url, &source.wire_api),
        source.detected_model_prices
    );
    assert_eq!(
        detected_prices_for_upstream(&source, " https://provider.test/v1 ", &source.wire_api),
        source.detected_model_prices
    );
    assert!(detected_prices_for_upstream(
        &source,
        "https://other-provider.test/v1",
        &source.wire_api
    )
    .is_empty());
    assert!(detected_prices_for_upstream(&source, &base_url, &WireApi::ChatCompletions).is_empty());
}

#[test]
fn source_priorities_are_applied_as_one_order() {
    let first = source_record();
    let mut second = source_record();
    second.id = "source-2".into();
    let mut sources = vec![first, second];

    apply_source_priorities(
        &mut sources,
        &BTreeMap::from([("source".into(), 2), ("source-2".into(), 1)]),
    )
    .unwrap();

    assert_eq!(sources[0].priority, 2);
    assert_eq!(sources[1].priority, 1);
    assert!(
        apply_source_priorities(&mut sources, &BTreeMap::from([("missing".into(), 3)]),).is_err()
    );
}

#[test]
fn mixed_binding_normalization_keeps_the_legacy_protocol_default_stable() {
    let mut source = source_record();
    source.protocol_bindings = vec![
        SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-native".into()],
        },
        SourceProtocolBinding {
            wire_api: WireApi::Responses,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["gpt-native".into()],
        },
    ];
    source.models = vec!["claude-native".into(), "gpt-native".into()];

    source.validate_protocol_bindings().unwrap();

    assert_eq!(source.wire_api, WireApi::Responses);
    assert!(!source
        .models_for_wire_api(WireApi::Responses)
        .unwrap()
        .is_empty());
    assert!(!source
        .models_for_wire_api(WireApi::Messages)
        .unwrap()
        .is_empty());
}

#[test]
fn source_probe_rejects_configuration_changes_but_ignores_runtime_status() {
    let before = source_record();
    let mut current = before.clone();
    current.last_test_status = Some("ok".into());
    assert!(source_probe_matches(&before, &current));

    current.models.push("model-b".into());
    assert!(!source_probe_matches(&before, &current));
}

#[test]
fn source_dispatch_fence_only_tracks_permission_and_transport_edits() {
    let before = source_record();
    let mut current = before.clone();
    current.priority += 1;
    current.weight += 1;
    current.name.push_str(" renamed");
    assert!(!source_dispatch_configuration_changed(&before, &current));

    let mut current = before.clone();
    current.in_pool = !current.in_pool;
    assert!(source_dispatch_configuration_changed(&before, &current));
    let mut current = before.clone();
    current.base_url = "https://another.example.test/v1".into();
    assert!(source_dispatch_configuration_changed(&before, &current));
    let mut current = before.clone();
    current.allowed_models.push("model-b".into());
    assert!(source_dispatch_configuration_changed(&before, &current));
}

#[test]
fn source_catalog_refreshes_for_pool_membership_changes() {
    let inside = source_record();
    let mut outside = inside.clone();
    outside.in_pool = false;

    assert!(source_catalog_visibility_changed(
        std::slice::from_ref(&inside),
        std::slice::from_ref(&outside)
    ));
    assert!(source_catalog_visibility_changed(
        std::slice::from_ref(&outside),
        std::slice::from_ref(&inside)
    ));
}
