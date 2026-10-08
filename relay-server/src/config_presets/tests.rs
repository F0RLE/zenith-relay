use super::{configuration_diff, merge_settings, normalize_preset};
use serde_json::json;
use std::collections::BTreeMap;
use zenith_relay_core::protocol::{
    ConfigurationPreset, ConfigurationPresetSettings, PresetQuotaPolicy, PresetRoutingPolicy,
    CONFIGURATION_PRESET_FORMAT,
};

fn settings() -> ConfigurationPresetSettings {
    ConfigurationPresetSettings {
        sources: Vec::new(),
        accounts: Vec::new(),
        routing: PresetRoutingPolicy {
            tool_policy: None,
            pool_routing: None,
            basis_points_enabled: false,
            max_retry_candidates: 3,
            default_service_tier: Default::default(),
            image_base_model: None,
        },
        quota: PresetQuotaPolicy {
            request_timeout_seconds: 20,
            account_proxy_required: false,
            common_proxy_id: None,
        },
        hidden_models: Vec::new(),
        model_price_overrides: Default::default(),
        model_reasoning_allowed_levels: Default::default(),
        model_reasoning_allowed_levels_present: true,
        model_service_tier_overrides: Default::default(),
        model_display_order: Vec::new(),
        model_service_tier_overrides_present: true,
        model_display_order_present: true,
    }
}

#[test]
fn diff_reports_only_changed_leaf() {
    let before = settings();
    let mut after = before.clone();
    after.routing.max_retry_candidates = 4;

    let changes = configuration_diff(&before, &after).unwrap();

    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "/routing/maxRetryCandidates");
}

#[test]
fn old_presets_preserve_tool_policy_and_explicit_reset_is_supported() {
    let mut current = settings();
    current.routing.tool_policy = Some(zenith_relay_core::ToolPolicy {
        mode: zenith_relay_core::ToolPolicyMode::Automatic,
    });
    let mut requested = settings();
    let merged = merge_settings(&current, &requested).unwrap();
    assert_eq!(merged.routing.tool_policy, current.routing.tool_policy);
    requested.routing.tool_policy = Some(Default::default());
    let merged = merge_settings(&current, &requested).unwrap();
    assert_eq!(merged.routing.tool_policy, Some(Default::default()));
}

#[test]
fn preset_ignores_legacy_routing_settings() {
    let mut preset = zenith_relay_core::protocol::ConfigurationPreset {
        format: zenith_relay_core::protocol::CONFIGURATION_PRESET_FORMAT.to_string(),
        schema_version: zenith_relay_core::protocol::CONFIGURATION_PRESET_SCHEMA_VERSION,
        settings: settings(),
    };
    let mut raw = serde_json::to_value(&preset).unwrap();
    raw["settings"]["routing"]["cooldownAfterFailures"] = serde_json::json!(0);
    raw["settings"]["routing"]["subscriptionPlanOrder"] = serde_json::json!(["not a valid\nplan"]);
    raw["settings"]["routing"]["routingStrategy"] = serde_json::json!("quota_highest");
    raw["settings"]["routing"]["keepLastCandidateAvailable"] = serde_json::json!(false);
    let imported: zenith_relay_core::protocol::ConfigurationPreset =
        serde_json::from_value(raw).unwrap();
    preset = super::normalize_preset(imported).unwrap();
    assert_eq!(preset.settings.routing.max_retry_candidates, 3);
    let serialized = serde_json::to_value(preset).unwrap();
    for legacy_field in [
        "cooldownAfterFailures",
        "subscriptionPlanOrder",
        "routingStrategy",
        "keepLastCandidateAvailable",
    ] {
        assert!(serialized["settings"]["routing"]
            .get(legacy_field)
            .is_none());
    }
}

#[test]
fn schema_two_omitted_reasoning_levels_preserve_current_configuration() {
    let mut current = settings();
    current.model_reasoning_allowed_levels =
        BTreeMap::from([("gpt-test".to_string(), vec!["medium".to_string()])]);
    current.model_service_tier_overrides = BTreeMap::from([(
        "gpt-test".to_string(),
        zenith_relay_core::DefaultServiceTier::Fast,
    )]);
    current.model_display_order = vec!["gpt-test".to_string()];
    let mut omitted_settings = serde_json::to_value(&current).unwrap();
    omitted_settings
        .as_object_mut()
        .unwrap()
        .remove("modelReasoningAllowedLevels");
    omitted_settings
        .as_object_mut()
        .unwrap()
        .remove("modelServiceTierOverrides");
    omitted_settings
        .as_object_mut()
        .unwrap()
        .remove("modelDisplayOrder");
    let omitted: ConfigurationPreset = serde_json::from_value(json!({
        "format": CONFIGURATION_PRESET_FORMAT,
        "schemaVersion": 2,
        "settings": omitted_settings,
    }))
    .unwrap();

    assert!(!omitted.settings.model_reasoning_allowed_levels_present);
    assert!(!omitted.settings.model_service_tier_overrides_present);
    assert!(!omitted.settings.model_display_order_present);
    assert!(serde_json::to_value(&omitted).unwrap()["settings"]
        .get("modelReasoningAllowedLevels")
        .is_none());
    let merged = merge_settings(&current, &normalize_preset(omitted).unwrap().settings).unwrap();
    assert_eq!(
        merged.model_reasoning_allowed_levels,
        current.model_reasoning_allowed_levels
    );
    assert!(merged.model_reasoning_allowed_levels_present);
    assert_eq!(
        merged.model_service_tier_overrides,
        current.model_service_tier_overrides
    );
    assert_eq!(merged.model_display_order, current.model_display_order);

    let mut explicit_settings = serde_json::to_value(&current).unwrap();
    explicit_settings["modelReasoningAllowedLevels"] = json!({});
    let explicit_empty: ConfigurationPreset = serde_json::from_value(json!({
        "format": CONFIGURATION_PRESET_FORMAT,
        "schemaVersion": 2,
        "settings": explicit_settings,
    }))
    .unwrap();

    assert!(
        explicit_empty
            .settings
            .model_reasoning_allowed_levels_present
    );
    assert!(merge_settings(
        &current,
        &normalize_preset(explicit_empty).unwrap().settings
    )
    .unwrap()
    .model_reasoning_allowed_levels
    .is_empty());
}
