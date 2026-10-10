use super::*;

#[test]
fn model_reasoning_summary_does_not_invent_unsupported_manual_levels() {
    let mut model = ModelSummary {
        enabled: true,
        protocol_routes: Vec::new(),
        codex_visible: true,
        codex_display_name: String::new(),
        id: "gpt-test".into(),
        member_count: 1,
        catalog_provider: None,
        catalog_source_model_id: None,
        catalog_canonical_model_id: None,
        catalog_family: None,
        catalog_name: None,
        catalog_release_date: None,
        catalog_last_updated: None,
        catalog_status: None,
        catalog_reasoning: None,
        catalog_reasoning_method: None,
        catalog_reasoning_effort_levels: Vec::new(),
        catalog_default_reasoning_effort: None,
        catalog_reasoning_budget_min_tokens: None,
        catalog_reasoning_budget_max_tokens: None,
        catalog_reasoning_budget_default_tokens: None,
        catalog_tool_call: None,
        catalog_structured_output: None,
        catalog_attachment: None,
        catalog_open_weights: None,
        catalog_input_modalities: Vec::new(),
        catalog_output_modalities: Vec::new(),
        catalog_context_limit: None,
        catalog_input_limit: None,
        catalog_output_limit: None,
        input_micro_usd_per_million: None,
        cached_input_micro_usd_per_million: None,
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: None,
        image_request_prices: Vec::new(),
        custom_price: false,
        reasoning_levels: Vec::new(),
        reasoning_supported_levels: Vec::new(),
        reasoning_allowed_levels: Vec::new(),
        reasoning_configurable: false,
        reasoning_manual_fallback: false,
        speed_supported: false,
        speed_tiers: Vec::new(),
        speed_tier: DefaultServiceTier::Standard,
        speed_configurable: false,
    };

    apply_model_reasoning_summary(
        &mut model,
        Some(vec!["high".into()]),
        Some(&["ultra".into()]),
        false,
    );
    assert!(model.reasoning_levels.is_empty());
    assert_eq!(model.reasoning_supported_levels, ["high"]);
    assert!(model.reasoning_allowed_levels.is_empty());
    assert!(!model.reasoning_configurable);
    assert!(!model.reasoning_manual_fallback);

    apply_model_reasoning_summary(&mut model, None, None, false);
    assert!(model.reasoning_levels.is_empty());
    assert!(model.reasoning_supported_levels.is_empty());
    assert!(model.reasoning_allowed_levels.is_empty());
    assert!(!model.reasoning_configurable);
    assert!(!model.reasoning_manual_fallback);

    apply_model_reasoning_summary(&mut model, Some(vec!["high".into()]), None, true);
    assert_eq!(model.reasoning_levels, ["high"]);
    assert_eq!(model.reasoning_supported_levels, ["high"]);
    assert_eq!(model.reasoning_allowed_levels, ["high"]);
    assert!(model.reasoning_configurable);
    assert!(!model.reasoning_manual_fallback);

    apply_model_reasoning_summary(&mut model, Some(Vec::new()), Some(&["max".into()]), true);
    assert!(model.reasoning_levels.is_empty());
    assert!(model.reasoning_supported_levels.is_empty());
    assert!(model.reasoning_allowed_levels.is_empty());
    assert!(!model.reasoning_configurable);
    assert!(!model.reasoning_manual_fallback);

    model.id = "gpt-5.6-terra".into();
    apply_model_reasoning_summary(&mut model, Some(Vec::new()), None, true);
    assert!(model.reasoning_supported_levels.is_empty());
    assert!(model.reasoning_allowed_levels.is_empty());
    assert!(!model.reasoning_manual_fallback);

    apply_model_reasoning_summary(&mut model, Some(vec!["ultra".into()]), None, true);
    assert_eq!(model.reasoning_supported_levels, ["ultra"]);
    assert_eq!(model.reasoning_allowed_levels, ["ultra"]);
    assert!(!model.reasoning_manual_fallback);

    model.id = "claude-fable-5-1".into();
    apply_model_reasoning_summary(&mut model, None, None, true);
    assert!(model.reasoning_supported_levels.is_empty());
    assert!(model.reasoning_allowed_levels.is_empty());
    assert!(!model.reasoning_configurable);
    assert!(!model.reasoning_manual_fallback);
}
#[test]
fn anthropic_modes_are_limited_to_provider_reported_levels() {
    let mut model = ModelSummary {
        enabled: true,
        protocol_routes: Vec::new(),
        codex_visible: true,
        codex_display_name: String::new(),
        id: "claude-opus-4-8".into(),
        member_count: 1,
        catalog_provider: None,
        catalog_source_model_id: None,
        catalog_canonical_model_id: None,
        catalog_family: None,
        catalog_name: None,
        catalog_release_date: None,
        catalog_last_updated: None,
        catalog_status: None,
        catalog_reasoning: None,
        catalog_reasoning_method: None,
        catalog_reasoning_effort_levels: Vec::new(),
        catalog_default_reasoning_effort: None,
        catalog_reasoning_budget_min_tokens: None,
        catalog_reasoning_budget_max_tokens: None,
        catalog_reasoning_budget_default_tokens: None,
        catalog_tool_call: None,
        catalog_structured_output: None,
        catalog_attachment: None,
        catalog_open_weights: None,
        catalog_input_modalities: Vec::new(),
        catalog_output_modalities: Vec::new(),
        catalog_context_limit: None,
        catalog_input_limit: None,
        catalog_output_limit: None,
        input_micro_usd_per_million: None,
        cached_input_micro_usd_per_million: None,
        cache_write_5m_micro_usd_per_million: None,
        cache_write_1h_micro_usd_per_million: None,
        output_micro_usd_per_million: None,
        image_request_prices: Vec::new(),
        custom_price: false,
        reasoning_levels: Vec::new(),
        reasoning_supported_levels: Vec::new(),
        reasoning_allowed_levels: Vec::new(),
        reasoning_configurable: false,
        reasoning_manual_fallback: false,
        speed_supported: false,
        speed_tiers: Vec::new(),
        speed_tier: DefaultServiceTier::Standard,
        speed_configurable: false,
    };
    apply_model_reasoning_summary(
        &mut model,
        Some(vec!["low".into(), "max".into()]),
        None,
        true,
    );
    assert_eq!(model.reasoning_supported_levels, ["low", "max"]);
    assert_eq!(model.reasoning_levels, ["low", "max"]);
}
#[test]
fn api_source_reasoning_route_requires_an_active_responses_source() {
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
    assert!(model_has_api_source_route(
        std::slice::from_ref(&source),
        "GPT-TEST"
    ));

    let mut unavailable = source.clone();
    unavailable.secret_available = false;
    assert!(!model_has_api_source_route(&[unavailable], "gpt-test"));

    let mut outside_pool = source;
    outside_pool.in_pool = false;
    assert!(!model_has_api_source_route(&[outside_pool], "gpt-test"));
}
