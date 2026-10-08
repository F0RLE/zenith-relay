use super::*;

#[test]
fn legacy_preset_without_pricing_identity_round_trips_without_new_fields() {
    let mut settings = ConfigurationPresetSettings {
        sources: vec![SourcePresetRule {
            legacy_protocol_mode: None,
            id: "source".into(),
            name: "Source".into(),
            base_url: "https://example.test/v1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            enabled: true,
            in_pool: true,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
        }],
        accounts: Vec::new(),
        routing: PresetRoutingPolicy {
            tool_policy: None,
            pool_routing: None,
            basis_points_enabled: false,
            max_retry_candidates: 3,
            default_service_tier: DefaultServiceTier::Standard,
            image_base_model: None,
        },
        quota: PresetQuotaPolicy {
            request_timeout_seconds: 30,
            account_proxy_required: false,
            common_proxy_id: None,
        },
        hidden_models: Vec::new(),
        model_price_overrides: BTreeMap::new(),
        model_reasoning_allowed_levels: BTreeMap::new(),
        model_service_tier_overrides: BTreeMap::new(),
        model_display_order: Vec::new(),
        model_reasoning_allowed_levels_present: true,
        model_service_tier_overrides_present: true,
        model_display_order_present: true,
    };
    let mut legacy = serde_json::to_value(ConfigurationPreset {
        format: CONFIGURATION_PRESET_FORMAT.into(),
        schema_version: CONFIGURATION_PRESET_SCHEMA_VERSION,
        settings: settings.clone(),
    })
    .unwrap();
    let source = legacy["settings"]["sources"][0].as_object_mut().unwrap();
    source.remove("pricingProvider");
    source.remove("officialProviderFamily");
    source.insert("protocolMode".into(), serde_json::json!("manual"));

    let decoded: ConfigurationPreset = serde_json::from_value(legacy).unwrap();
    assert_eq!(decoded.settings.sources[0].pricing_provider, None);
    assert_eq!(decoded.settings.sources[0].official_provider_family, None);
    assert_eq!(
        decoded.settings.sources[0].legacy_protocol_mode.as_deref(),
        Some("manual")
    );
    settings.sources[0].pricing_provider = None;
    settings.sources[0].official_provider_family = None;
    settings.sources[0].legacy_protocol_mode = Some("manual".into());
    assert_eq!(decoded.settings.sources, settings.sources);

    let encoded = serde_json::to_value(decoded).unwrap();
    let source = encoded["settings"]["sources"][0].as_object().unwrap();
    assert!(!source.contains_key("pricingProvider"));
    assert!(!source.contains_key("officialProviderFamily"));
    assert!(!source.contains_key("protocolMode"));
}
#[test]
fn configuration_preset_validation_rejects_untrusted_identity_and_endpoint() {
    let mut preset = valid_configuration_preset();
    preset.format = "other-product".into();
    assert!(normalize_configuration_preset(preset).is_err());

    let mut preset = valid_configuration_preset();
    preset.schema_version = CONFIGURATION_PRESET_SCHEMA_VERSION + 1;
    assert!(normalize_configuration_preset(preset).is_err());

    let mut preset = valid_configuration_preset();
    preset.settings.sources[0].base_url = "file:///not-an-api".into();
    assert!(normalize_configuration_preset(preset).is_err());
}
#[test]
fn configuration_preset_validation_normalizes_source_policy() {
    let mut preset = valid_configuration_preset();
    let source = &mut preset.settings.sources[0];
    source.base_url = " https://example.test/v1/ ".into();
    source.allowed_models = vec!["gpt-test".into(), "GPT-TEST".into()];

    let normalized = normalize_configuration_preset(preset).unwrap();
    assert_eq!(
        normalized.settings.sources[0].base_url,
        "https://example.test/v1"
    );
    assert_eq!(normalized.settings.sources[0].allowed_models, ["gpt-test"]);
}
#[test]
fn legacy_preset_rotation_upgrades_without_consent_or_losing_member_limits() {
    let mut current = valid_configuration_preset().settings;
    current.routing.pool_routing = Some(current.resolved_pool_routing());
    let mut requested = current.clone();
    let legacy_routing = requested.routing.pool_routing.as_mut().unwrap();
    legacy_routing.version = 1;
    legacy_routing.mode = crate::PoolRoutingMode::Smart;
    legacy_routing.members.reverse();
    for member in &mut legacy_routing.members {
        member.weight = 7;
        member.max_concurrency = 4;
    }
    let expected_members = legacy_routing.members.clone();
    let merged = merge_configuration_preset_settings(&current, &requested).unwrap();
    let policy = merged.routing.pool_routing.unwrap();
    assert!(policy.is_current_rotation());
    assert_eq!(policy.mode, crate::PoolRoutingMode::Automatic);
    assert_eq!(policy.members, expected_members);
    requested.routing.pool_routing = None;
    assert_eq!(
        merge_configuration_preset_settings(&current, &requested)
            .unwrap()
            .routing
            .pool_routing,
        current.routing.pool_routing
    );
}
#[test]
fn sparse_configuration_preset_keeps_newer_model_policy() {
    let current = valid_configuration_preset().settings;
    let mut current = ConfigurationPresetSettings {
        model_reasoning_allowed_levels: BTreeMap::from([("gpt-test".into(), vec!["high".into()])]),
        model_service_tier_overrides: BTreeMap::from([(
            "gpt-test".into(),
            DefaultServiceTier::Fast,
        )]),
        model_display_order: vec!["gpt-test".into()],
        ..current
    };
    current.model_reasoning_allowed_levels_present = true;
    current.model_service_tier_overrides_present = true;
    current.model_display_order_present = true;
    let mut sparse = current.clone();
    sparse.model_reasoning_allowed_levels.clear();
    sparse.model_service_tier_overrides.clear();
    sparse.model_display_order.clear();
    sparse.model_reasoning_allowed_levels_present = false;
    sparse.model_service_tier_overrides_present = false;
    sparse.model_display_order_present = false;

    let merged = merge_configuration_preset_settings(&current, &sparse).unwrap();

    assert_eq!(
        merged.model_reasoning_allowed_levels,
        current.model_reasoning_allowed_levels
    );
    assert_eq!(
        merged.model_service_tier_overrides,
        current.model_service_tier_overrides
    );
    assert_eq!(merged.model_display_order, current.model_display_order);
}
#[test]
fn resolved_configuration_preset_rejects_duplicate_local_members() {
    let mut settings = valid_configuration_preset().settings;
    settings.sources.push(settings.sources[0].clone());

    assert!(validate_resolved_configuration_preset_members(&settings).is_err());
}
#[test]
fn legacy_preset_routing_fields_are_read_but_not_exported() {
    let policy: PresetRoutingPolicy = serde_json::from_str(
            r#"{"maxRetryCandidates":3,"routingStrategy":"adaptive","subscriptionPlanOrder":["business"],"cooldownAfterFailures":7,"keepLastCandidateAvailable":false,"defaultServiceTier":"standard","imageBaseModel":null}"#,
        )
        .unwrap();
    assert_eq!(policy.max_retry_candidates, 3);
    let saved = serde_json::to_value(policy).unwrap();
    for legacy_field in [
        "cooldownAfterFailures",
        "keepLastCandidateAvailable",
        "routingStrategy",
        "subscriptionPlanOrder",
    ] {
        assert!(
            saved.get(legacy_field).is_none(),
            "{legacy_field} must not be exported"
        );
    }
    let mut unsupported = saved;
    unsupported["unknownRoutingControl"] = serde_json::json!(true);
    assert!(serde_json::from_value::<PresetRoutingPolicy>(unsupported).is_err());
}
