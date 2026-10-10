use super::*;

#[test]
fn numbered_gpt_fallback_labels_match_the_compact_codex_picker() {
    for (model, expected) in [
        ("gpt-6-astra", "6 Astra"),
        ("gpt-5.6-sol", "5.6 Sol"),
        ("gpt-5.6-terra", "5.6 Terra"),
        ("local/gpt-6-astra", "6 Astra"),
        ("GPT-6-Astra", "6 Astra"),
        ("gpt-123-future", "123 Future"),
        ("gpt-124.7-next", "124.7 Next"),
        ("next-family-synthetic", "Next Family Synthetic"),
        ("gpt-future", "GPT Future"),
        ("gpt-", "GPT"),
        ("vendor/claude-opus-4-8", "Claude Opus 4.8"),
    ] {
        assert_eq!(codex_model_display_name(model), expected, "{model}");
    }
}

#[test]
fn relay_aliases_are_exact_and_media_models_stay_out_of_codex() {
    let model = "vendor/claude-opus-4-8";
    let alias = codex_model_alias(model);
    assert_eq!(decode_codex_model_alias(&alias).as_deref(), Some(model));
    assert_eq!(codex_model_display_name(model), "Claude Opus 4.8");
    assert!(codex_model_is_picker_eligible(model));
    assert!(!codex_model_is_picker_eligible("gpt-image-2"));
    assert!(!codex_model_is_picker_eligible(
        "gpt-6-astra-degrade2-luna-1p-codexswic-ev3"
    ));
    assert!(codex_model_is_picker_eligible_for(
        "gpt-6-astra-degrade2-luna-1p-codexswic-ev3",
        false
    ));
    assert!(decode_codex_model_alias("zenith/not-base64!").is_none());
}

#[test]
fn generated_picker_order_preserves_discovery_order_without_metadata() {
    let models = crate::normalize_model_ids([
        "vendor/glm-5.2",
        "vendor/grok-4.5",
        "vendor/gemini-3.6-flash",
        "vendor/claude-opus-4-8",
        "gpt-5.4-mini",
        "vendor/gpt-5.4",
        "gpt-5.5",
        "gpt-5.6-luna",
        "gpt-5.6-terra",
        "gpt-5.6-sol",
        "vendor/unknown-model",
    ]);

    assert_eq!(
        models,
        [
            "vendor/glm-5.2",
            "vendor/grok-4.5",
            "vendor/gemini-3.6-flash",
            "vendor/claude-opus-4-8",
            "gpt-5.4-mini",
            "vendor/gpt-5.4",
            "gpt-5.5",
            "gpt-5.6-luna",
            "gpt-5.6-terra",
            "gpt-5.6-sol",
            "vendor/unknown-model",
        ]
    );
}

#[test]
fn catalog_aliases_do_not_change_the_model_identity() {
    let alias = codex_model_alias("gpt-5.6-sol");
    assert_eq!(
        decode_codex_model_alias(&alias).as_deref(),
        Some("gpt-5.6-sol")
    );
}

#[test]
fn routed_models_strip_native_only_selectors_from_template() {
    let template = json!({
        "base_instructions": "native Codex instructions",
        "model_messages": {"instructions_template": "native template"},
        "tool_mode": "code_mode",
        "multi_agent_version": "v2",
        "default_reasoning_level": "medium",
        "supported_reasoning_levels": [{"effort": "low", "description": "Low"}],
        "web_search_tool_type": "text_and_image",
        "use_responses_lite": true,
    });
    let catalog_entry =
        routed_codex_catalog_entry(template.as_object(), "vendor/claude-fable-5", 1_000, None);

    let instructions = catalog_entry["base_instructions"].as_str().unwrap();
    assert_eq!(instructions, super::entry::ROUTED_CODEX_BASE_INSTRUCTIONS);
    assert!(instructions.contains("apply_patch"));
    assert!(instructions.contains("PowerShell"));
    assert!(instructions.contains("macOS and Linux"));
    assert!(!instructions.contains("native Codex instructions"));
    assert!(catalog_entry.get("model_messages").is_none());
    assert!(catalog_entry.get("tool_mode").is_none());
    assert!(catalog_entry.get("multi_agent_version").is_none());
    assert!(catalog_entry.get("default_reasoning_level").is_none());
    assert_eq!(catalog_entry["supported_reasoning_levels"], json!([]));
    assert_eq!(catalog_entry["web_search_tool_type"], "text");
    assert!(catalog_entry.get("use_responses_lite").is_none());
    assert!(catalog_entry.get("service_tiers").is_none());
    assert_eq!(catalog_entry["supports_reasoning_summaries"], false);
    assert_eq!(catalog_entry["supports_parallel_tool_calls"], false);
    assert_eq!(catalog_entry["input_modalities"], json!(["text", "image"]));
}

#[test]
fn api_models_use_medium_when_provider_default_is_ultra() {
    let template = json!({
        "default_reasoning_level": "ultra",
        "supported_reasoning_levels": [
            {"effort": "low", "description": "Low"},
            {"effort": "medium", "description": "Medium"},
            {"effort": "ultra", "description": "Maximum reasoning with automatic task delegation"}
        ]
    });

    let catalog_entry = normalize_upstream_codex_catalog_entry(
        template.as_object().unwrap(),
        "vendor/model",
        1_000,
        None,
    )
    .expect("API catalog entry");

    assert_eq!(catalog_entry["default_reasoning_level"], "medium");
    assert_eq!(
        catalog_entry["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        catalog_entry["supported_reasoning_levels"][2]["description"],
        "ultra"
    );
}

#[test]
fn api_models_do_not_inherit_ultra_when_medium_is_unavailable() {
    let template = json!({
        "default_reasoning_level": "ultra",
        "supported_reasoning_levels": [
            {"effort": "low", "description": "Low"},
            {"effort": "high", "description": "High"},
            {"effort": "ultra", "description": "Ultra"}
        ]
    });

    let catalog_entry = normalize_upstream_codex_catalog_entry(
        template.as_object().unwrap(),
        "vendor/model",
        1_000,
        None,
    )
    .expect("API catalog entry");

    assert!(catalog_entry.get("default_reasoning_level").is_none());
}

#[test]
fn native_models_keep_bare_slug_and_upstream_capabilities() {
    let template = json!({
        "slug": "gpt-5.6-sol",
        "display_name": "GPT-5.6 Sol",
        "base_instructions": "native Codex instructions",
        "shell_type": "default",
        "visibility": "list",
        "supported_in_api": true,
        "priority": 10,
        "default_reasoning_level": "high",
        "supported_reasoning_levels": [{"effort": "low", "description": "Low"}],
        "service_tiers": [{
            "id": "priority",
            "name": "Fast",
            "description": "Native fast tier"
        }],
        "default_service_tier": "priority",
        "additional_speed_tiers": ["priority"],
        "supports_reasoning_summary_parameter": true,
        "supports_reasoning_summaries": true,
        "default_reasoning_summary": "detailed",
        "support_verbosity": true,
        "default_verbosity": "medium",
        "supports_parallel_tool_calls": true,
        "supports_image_detail_original": true,
        "supports_search_tool": true,
        "use_responses_lite": true,
        "input_modalities": ["text"],
        "experimental_supported_tools": [],
        "apply_patch_tool_type": "freeform",
        "truncation_policy": {"mode": "tokens", "limit": 10000},
        "context_window": 128000,
        "max_context_window": 120000,
        "auto_compact_token_limit": 110000,
        "native_setting": "keep-me",
    });
    let catalog_entry = normalize_native_codex_catalog_entry(
        template.as_object().unwrap(),
        "gpt-5.6-sol",
        1_000,
        Some(1_000_000),
    )
    .unwrap();

    assert_eq!(catalog_entry["slug"], "gpt-5.6-sol");
    assert_eq!(catalog_entry["default_reasoning_level"], "high");
    assert_eq!(catalog_entry["service_tiers"][0]["id"], "priority");
    assert_eq!(catalog_entry["supports_parallel_tool_calls"], true);
    assert_eq!(catalog_entry["use_responses_lite"], true);
    assert_eq!(catalog_entry["input_modalities"], json!(["text"]));
    assert_eq!(catalog_entry["context_window"], 128_000);
    assert_eq!(catalog_entry["max_context_window"], 120_000);
    assert_eq!(catalog_entry["auto_compact_token_limit"], 110_000);
    assert_eq!(catalog_entry["native_setting"], "keep-me");
}

#[test]
fn native_models_do_not_inherit_routed_image_defaults() {
    let template = json!({
        "slug": "gpt-native",
        "display_name": "GPT Native",
    });

    let catalog_entry = normalize_native_codex_catalog_entry(
        template.as_object().unwrap(),
        "gpt-native",
        1_000,
        None,
    )
    .unwrap();

    assert!(catalog_entry.get("input_modalities").is_none());
    assert!(catalog_entry.get("context_window").is_none());
    assert!(catalog_entry.get("max_context_window").is_none());
    assert!(catalog_entry.get("auto_compact_token_limit").is_none());
}

#[test]
fn native_models_do_not_synthesize_missing_context_fields() {
    let template = json!({
        "slug": "gpt-native",
        "display_name": "GPT Native",
        "context_window": 128_000,
    });

    let catalog_entry = normalize_native_codex_catalog_entry(
        template.as_object().unwrap(),
        "gpt-native",
        1_000,
        None,
    )
    .unwrap();

    assert_eq!(catalog_entry["context_window"], 128_000);
    assert!(catalog_entry.get("max_context_window").is_none());
    assert!(catalog_entry.get("auto_compact_token_limit").is_none());
    assert!(catalog_entry
        .get("effective_context_window_percent")
        .is_none());
}

#[test]
fn native_models_keep_an_arbitrary_upstream_slug_over_the_routing_name() {
    let template = json!({
        "slug": "vendor/future-model-2026-08",
        "display_name": "Future upstream name"
    });

    let catalog_entry = normalize_native_codex_catalog_entry(
        template.as_object().unwrap(),
        "configured-alias",
        1_000,
        None,
    )
    .unwrap();

    assert_eq!(catalog_entry["slug"], "vendor/future-model-2026-08");
    assert_eq!(catalog_entry["display_name"], "Future upstream name");
    assert!(!catalog_entry["slug"]
        .as_str()
        .unwrap()
        .starts_with("zenith/"));
}

#[test]
fn routed_models_publish_known_limits_without_inheriting_template_context_policy() {
    let template = json!({
        "context_window": 272_000,
        "max_context_window": 272_000,
        "auto_compact_token_limit": 244_800,
        "effective_context_window_percent": 90,
    });

    let advertised =
        routed_codex_catalog_entry(template.as_object(), "vendor/large", 1_000, Some(1_000_000));
    assert!(advertised.get("context_window").is_none());
    assert_eq!(advertised["max_context_window"], 1_000_000);
    assert!(advertised.get("auto_compact_token_limit").is_none());
    assert!(advertised.get("effective_context_window_percent").is_none());

    let unknown = routed_codex_catalog_entry(template.as_object(), "vendor/unknown", 1_001, None);
    assert!(unknown.get("context_window").is_none());
    assert!(unknown.get("max_context_window").is_none());
    assert!(unknown.get("auto_compact_token_limit").is_none());
    assert!(unknown.get("effective_context_window_percent").is_none());
}

#[test]
fn api_maximum_does_not_replace_codex_default_context_window() {
    let mut model = routed_codex_catalog_entry(None, "gpt-future", 1_000, None);
    publish_routed_codex_context(&mut model, Some(1_050_000), Some(272_000));
    assert_eq!(model["context_window"], 272_000);
    assert_eq!(model["max_context_window"], 1_050_000);
    assert!(model.get("auto_compact_token_limit").is_none());
    assert!(model.get("effective_context_window_percent").is_none());

    publish_routed_codex_context(&mut model, Some(128_000), Some(272_000));
    assert_eq!(model["context_window"], 128_000);
    assert_eq!(model["max_context_window"], 128_000);

    publish_routed_codex_context(&mut model, Some(1_050_000), None);
    assert!(model.get("context_window").is_none());
    assert_eq!(model["max_context_window"], 1_050_000);
}

#[test]
fn routed_models_publish_codex_required_truncation_policy() {
    let catalog_entry = routed_codex_catalog_entry(None, "vendor/large", 1_000, Some(1_000_000));

    assert_eq!(
        catalog_entry.get("truncation_policy"),
        Some(&json!({"mode": "tokens", "limit": 10000}))
    );
    assert!(catalog_entry.get("context_window").is_none());
    assert_eq!(catalog_entry["max_context_window"], 1_000_000);
}

#[test]
fn strict_catalog_validation_rejects_poisoned_or_incomplete_rows() {
    let valid = routed_codex_catalog_entry(None, "vendor/model", 1_000, None);
    assert!(codex_catalog_entry_is_compatible(&valid));

    let mut missing_required = valid.clone();
    missing_required
        .as_object_mut()
        .unwrap()
        .remove("supported_reasoning_levels");
    assert!(!codex_catalog_entry_is_compatible(&missing_required));

    let mut poisoned = valid;
    poisoned["input_modalities"] = json!(["text", "video"]);
    assert!(!codex_catalog_entry_is_compatible(&poisoned));
}

#[test]
fn official_codex_ultra_requires_exact_identity_and_routable_child_effort() {
    let official = json!({
        "slug": "gpt-future",
        "supported_reasoning_levels": [{"effort": "max"}, {"effort": "ultra"}],
        "multi_agent_version": "v2",
        "multi_agent_reasoning_effort": "xhigh",
        "base_instructions": "never inherit this"
    });
    let mut catalog_entry = routed_codex_catalog_entry(None, "gpt-future", 1_000, None);
    catalog_entry["supported_reasoning_levels"] = json!([
        {"effort": "xhigh", "description": "xhigh"},
        {"effort": "max", "description": "max"}
    ]);
    assert!(!apply_codex_ultra_from_official_model(
        &mut catalog_entry,
        &official,
        "gpt-other"
    ));
    assert!(!apply_codex_ultra_from_official_model(
        &mut catalog_entry,
        &official,
        "gpt-future-other"
    ));
    assert!(apply_codex_ultra_from_official_model(
        &mut catalog_entry,
        &official,
        "gpt-future"
    ));
    assert_eq!(
        catalog_entry["supported_reasoning_levels"][2]["effort"],
        "ultra"
    );
    assert_eq!(catalog_entry["multi_agent_version"], "v2");
    assert_eq!(catalog_entry["multi_agent_reasoning_effort"], "xhigh");
    assert_ne!(catalog_entry["base_instructions"], "never inherit this");
    assert!(codex_catalog_entry_is_compatible(&catalog_entry));

    let mut missing_child = routed_codex_catalog_entry(None, "gpt-future", 1_000, None);
    missing_child["supported_reasoning_levels"] = json!([{"effort": "max"}]);
    assert!(!apply_codex_ultra_from_official_model(
        &mut missing_child,
        &official,
        "gpt-future"
    ));
    assert_eq!(
        missing_child["supported_reasoning_levels"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
