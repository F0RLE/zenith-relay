use super::*;

#[test]
fn pool_snapshot_configuration_keeps_model_speed_without_a_running_listener() {
    let source = SourceSummary {
        resolved_protocol_bindings: None,
        id: "source_1".into(),
        name: "Synthetic".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        operational_status: OperationalStatus::Rotation,
        base_url: "https://example.test/v1".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_bindings: Vec::new(),
        protocol_config: crate::SourceProtocolConfig::default(),
        models: vec!["gpt-test".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        api_equivalent: ApiEquivalentSummary::default(),
        secret_available: true,
        last_error_code: None,
        refresh_revision: None,
        refresh_state: SourceRefreshState::default(),
        provider_stats: None,
    };
    let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
    let price_overrides = BTreeMap::from([(
        "gpt-test".to_string(),
        ApiModelPriceOverride {
            input_micro_usd_per_million: 12,
            cached_input_micro_usd_per_million: None,
            cache_write_5m_micro_usd_per_million: Some(18),
            cache_write_1h_micro_usd_per_million: Some(9),
            output_micro_usd_per_million: 34,
        },
    )]);
    let reasoning_allowed_levels =
        BTreeMap::from([("gpt-test".to_string(), vec!["high".to_string()])]);
    let service_tier_overrides =
        BTreeMap::from([("gpt-test".to_string(), DefaultServiceTier::Fast)]);

    apply_pool_model_configuration(
        &mut models,
        std::slice::from_ref(&source),
        &[],
        &price_overrides,
        &reasoning_allowed_levels,
        &service_tier_overrides,
        None,
    );

    assert_eq!(models.len(), 1);
    let model = &models[0];
    assert!(model.custom_price);
    assert_eq!(model.input_micro_usd_per_million, Some(12));
    assert_eq!(model.cached_input_micro_usd_per_million, None);
    assert_eq!(model.cache_write_5m_micro_usd_per_million, None);
    assert_eq!(model.cache_write_1h_micro_usd_per_million, None);
    assert_eq!(model.output_micro_usd_per_million, Some(34));
    assert!(model.reasoning_levels.is_empty());
    assert_eq!(model.speed_tier, DefaultServiceTier::Fast);
    assert!(model.speed_supported);
    assert!(model.speed_configurable);
    assert_eq!(pool_candidate_count(&[source], &[]), 1);
}
#[test]
fn pool_snapshot_applies_the_model_speed_policy_without_participant_evidence() {
    let source = SourceSummary {
        resolved_protocol_bindings: None,
        id: "source_1".into(),
        name: "Synthetic".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        operational_status: OperationalStatus::Rotation,
        base_url: "https://example.test/v1".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_bindings: Vec::new(),
        protocol_config: crate::SourceProtocolConfig::default(),
        models: vec!["gpt-test".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        api_equivalent: ApiEquivalentSummary::default(),
        secret_available: true,
        last_error_code: None,
        refresh_revision: None,
        refresh_state: SourceRefreshState::default(),
        provider_stats: None,
    };
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(ProviderSource {
            id: source.id.clone(),
            name: source.name.clone(),
            base_url: source.base_url.clone(),
            api_key: "synthetic-upstream-secret".into(),
            wire_api: source.wire_api,
            models: source.models.clone(),
        })],
        vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
            id: "key_1".into(),
            secret: "synthetic-local-secret".into(),
        })],
        GatewayRuntimeOptions {
            default_service_tier: DefaultServiceTier::Fast,
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);

    apply_pool_model_configuration(
        &mut models,
        std::slice::from_ref(&source),
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        Some(&runtime),
    );

    assert_eq!(models[0].speed_tier, DefaultServiceTier::Fast);
    assert!(models[0].speed_supported);
    assert!(models[0].speed_configurable);
}
#[test]
fn pool_model_summaries_include_the_runtime_messages_bridge() {
    let source = SourceSummary {
        resolved_protocol_bindings: None,
        id: "source_1".into(),
        name: "Mixed source".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        operational_status: OperationalStatus::Rotation,
        base_url: "https://example.test/v1".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: crate::SourceProtocolConfig::default(),
        protocol_bindings: vec![
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-routed".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-native".into()],
            },
        ],
        models: vec!["gpt-routed".into(), "claude-native".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        api_equivalent: ApiEquivalentSummary::default(),
        secret_available: true,
        last_error_code: None,
        refresh_revision: None,
        refresh_state: SourceRefreshState::default(),
        provider_stats: None,
    };

    let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
    apply_pool_model_configuration(
        &mut models,
        std::slice::from_ref(&source),
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::from([("claude-native".to_string(), DefaultServiceTier::Fast)]),
        None,
    );

    assert_eq!(
        models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        ["gpt-routed", "claude-native"]
    );
    assert!(models[0].speed_supported);
    assert!(!models[1].speed_supported);
    assert_eq!(models[1].speed_tier, DefaultServiceTier::Standard);
}
