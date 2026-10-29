use super::super::*;
use crate::model_metadata::reasoning::enrich_reasoning_metadata;

fn catalog(raw: &str) -> ModelMetadataCatalog {
    ModelMetadataCatalog::from_models_dev_json(raw).unwrap()
}

#[test]
fn equivalent_reference_records_publish_reasoning_without_merging_identity() {
    use serde_json::json;

    let id = "synthetic-future";
    let first = format!("source-a/{id}");
    let second = format!("source-b/{id}");
    let record = json!({
        "name": "Synthetic Future",
        "family": "future",
        "reasoning": true,
        "reasoning_effort_levels": ["low", "high", "max"],
        "default_reasoning_effort": "high"
    });
    let mut records = json!({});
    records[&first] = record.clone();
    records[&second] = record;

    let metadata = catalog(&records.to_string());
    assert!(metadata.resolve(id).is_some());
    assert_eq!(metadata.resolve(&first).unwrap().source_model_id, first);
    assert_eq!(metadata.resolve(&second).unwrap().source_model_id, second);
    let mut card = json!({"slug": id});
    metadata.apply_codex_capabilities(id, &mut card);
    assert_eq!(card["default_reasoning_level"], "high");
    assert_eq!(
        card["supported_reasoning_levels"],
        json!([
            {"effort": "low", "description": "low"},
            {"effort": "high", "description": "high"},
            {"effort": "max", "description": "max"}
        ])
    );

    records[&second]["reasoning_effort_levels"] = json!(["high"]);
    assert!(catalog(&records.to_string()).resolve(id).is_none());

    records[&second] = records[&first].clone();
    let third = format!("other/{id}");
    records[&third] = records[&first].clone();
    assert!(catalog(&records.to_string()).resolve(id).is_some());

    records[&third]["name"] = json!("Different Model");
    assert!(catalog(&records.to_string()).resolve(id).is_none());
}

#[test]
fn equivalent_records_with_different_canonical_ids_remain_ambiguous() {
    use serde_json::json;

    let mut records = json!({
        "source-a/model": {
            "name": "Model",
            "family": "family",
            "canonical_model_id": "provider/model-a",
            "reasoning": true
        },
        "source-b/model": {
            "name": "Model",
            "family": "family",
            "canonical_model_id": "provider/model-b",
            "reasoning": true
        }
    });
    assert!(catalog(&records.to_string()).resolve("model").is_none());

    records["source-b/model"]["canonical_model_id"] = json!("provider/model-a");
    assert!(catalog(&records.to_string()).resolve("model").is_some());
}

#[test]
fn keeps_openrouter_reasoning_separate_from_first_party_levels() {
    let models = serde_json::json!({
        "openai/gpt-test": {"reasoning": true}
    });
    let openrouter = serde_json::json!({"data": [{
        "id": "openai/gpt-test",
        "supported_parameters": ["reasoning", "reasoning_effort"],
        "reasoning": {"effort": ["low", "high"]}
    }]});
    let litellm = serde_json::json!({
        "openai/gpt-test": {
            "supports_low_reasoning_effort": true,
            "supports_medium_reasoning_effort": true,
            "supports_high_reasoning_effort": true
        }
    });
    let merged = enrich_reasoning_metadata(&models, Some(&openrouter), Some(&litellm));
    assert_eq!(
        merged["openrouter/openai/gpt-test"]["reasoning_effort_levels"],
        serde_json::json!(["low", "high"])
    );
    assert_eq!(
        merged["openai/gpt-test"]["reasoning_effort_levels"],
        serde_json::json!(["low", "medium", "high"])
    );
}

#[test]
fn falls_back_to_litellm_when_openrouter_has_no_exact_levels() {
    let models = serde_json::json!({"openai/gpt-test": {"reasoning": true}});
    let openrouter = serde_json::json!({"data": [{
        "id": "openai/gpt-test",
        "supported_parameters": ["reasoning_effort"]
    }]});
    let litellm = serde_json::json!({
        "openai/gpt-test": {
            "supports_minimal_reasoning_effort": true,
            "supports_high_reasoning_effort": true
        }
    });
    let merged = enrich_reasoning_metadata(&models, Some(&openrouter), Some(&litellm));
    assert_eq!(
        merged["openai/gpt-test"]["reasoning_effort_levels"],
        serde_json::json!(["minimal", "high"])
    );
}

#[test]
fn complete_scoped_effort_enum_wins_over_partial_litellm_flags() {
    let models = serde_json::json!({"provider/synthetic": {"reasoning": true}});
    let details = serde_json::json!({"provider": {"models": {"synthetic": {
        "reasoning": true,
        "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high", "xhigh", "max"]}]
    }}}});
    let litellm = serde_json::json!({"synthetic": {
        "litellm_provider": "provider",
        "supports_reasoning": true,
        "supports_xhigh_reasoning_effort": true,
        "supports_max_reasoning_effort": true,
        "supports_none_reasoning_effort": false
    }});
    let merged = reasoning::enrich_reasoning_metadata_with_models_dev_details(
        models,
        Some(details),
        None,
        Some(litellm),
    );
    let metadata = catalog(&merged.to_string());
    assert_eq!(
        metadata.reasoning_levels_for("provider/synthetic"),
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(
        merged["provider/synthetic"]["reasoning_source"],
        "models_dev"
    );
}

#[test]
fn litellm_nested_effort_enum_survives_absent_or_partial_flags() {
    let models = serde_json::json!({"provider/synthetic": {"reasoning": true}});
    for extra_flag in [false, true] {
        let mut litellm = serde_json::json!({"provider/synthetic": {
            "reasoning": {"type": "effort", "values": ["low", "medium", "high"]}
        }});
        if extra_flag {
            litellm["provider/synthetic"]["supports_high_reasoning_effort"] = true.into();
        }
        let merged = enrich_reasoning_metadata(&models, None, Some(&litellm));
        assert_eq!(
            catalog(&merged.to_string()).reasoning_levels_for("provider/synthetic"),
            ["low", "medium", "high"]
        );
    }
}

#[test]
fn direct_metadata_prefers_declared_effort_enums_over_flags() {
    for declaration in [
        serde_json::json!({"reasoning": {"type": "effort", "values": ["low", "medium", "high"]}}),
        serde_json::json!({"reasoning": true, "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}]}),
    ] {
        let mut record = declaration;
        record["supports_high_reasoning_effort"] = true.into();
        let metadata = catalog(&serde_json::json!({"provider/synthetic": record}).to_string());
        assert_eq!(
            metadata.reasoning_levels_for("provider/synthetic"),
            ["low", "medium", "high"]
        );
    }
}

#[test]
fn primary_effort_enum_and_default_survive_partial_enrichment() {
    let models = serde_json::json!({"provider/synthetic": {
        "reasoning": true,
        "reasoning_method": "adaptive",
        "reasoning_effort_levels": ["low", "medium", "high"],
        "default_reasoning_effort": "high"
    }});
    let litellm = serde_json::json!({"provider/synthetic": {
        "supports_high_reasoning_effort": true
    }});
    let merged = enrich_reasoning_metadata(&models, None, Some(&litellm));
    let capabilities = catalog(&merged.to_string()).capabilities_for("provider/synthetic");
    assert_eq!(
        capabilities.reasoning_effort_levels,
        ["low", "medium", "high"]
    );
    assert_eq!(
        capabilities.default_reasoning_effort.as_deref(),
        Some("high")
    );
    assert_eq!(
        capabilities.reasoning_method,
        Some(ReasoningMethod::Adaptive)
    );
}

#[test]
fn scalar_effort_is_not_an_exhaustive_enum() {
    let models = serde_json::json!({"provider/synthetic": {"reasoning": true}});
    let details = serde_json::json!({"provider": {"models": {"synthetic": {
        "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}]
    }}}});
    let litellm = serde_json::json!({"provider/synthetic": {
        "reasoning": {"effort": "high"}
    }});
    let merged = reasoning::enrich_reasoning_metadata_with_models_dev_details(
        models,
        Some(details),
        None,
        Some(litellm),
    );
    assert_eq!(
        catalog(&merged.to_string()).reasoning_levels_for("provider/synthetic"),
        ["low", "medium", "high"]
    );

    let direct = serde_json::json!({"provider/synthetic": {
        "reasoning": {"effort": "max"},
        "reasoning_options": [{"type": "effort", "values": ["low", "medium", "high"]}]
    }});
    assert_eq!(
        catalog(&direct.to_string()).reasoning_levels_for("provider/synthetic"),
        ["low", "medium", "high"]
    );
}

#[test]
fn explicit_unsupported_reasoning_is_not_revived_by_effort_flags() {
    for reasoning in [
        serde_json::json!(false),
        serde_json::json!({"supported": false}),
    ] {
        let models = serde_json::json!({"provider/synthetic": {
            "reasoning": reasoning,
            "reasoning_effort_levels": ["low", "medium", "high"],
            "supports_high_reasoning_effort": true
        }});
        let merged = enrich_reasoning_metadata(&models, None, None);
        let capabilities = catalog(&merged.to_string()).capabilities_for("provider/synthetic");
        assert_eq!(capabilities.reasoning, Some(false));
        assert!(capabilities.reasoning_effort_levels.is_empty());
    }
    let unsupported = serde_json::json!({"provider/synthetic": {
        "reasoning": {"effort": ["low", "high"]},
        "supports_reasoning": false,
        "supports_high_reasoning_effort": true
    }});
    let capabilities = catalog(&unsupported.to_string()).capabilities_for("provider/synthetic");
    assert_eq!(capabilities.reasoning, Some(false));
    assert!(capabilities.reasoning_effort_levels.is_empty());
}

#[test]
fn does_not_invent_levels_from_reasoning_boolean() {
    let models = serde_json::json!({"openai/gpt-test": {"reasoning": true}});
    let merged = enrich_reasoning_metadata(&models, None, None);
    assert!(
        merged["openai/gpt-test"]
            .get("reasoning_effort_levels")
            .is_none()
    );
}

#[test]
fn parses_reasoning_method_and_filters_unrecognized_efforts() {
    let models = serde_json::json!({"provider/model": {"reasoning": true}});
    let openrouter = serde_json::json!({"data": [{
        "id": "provider/model",
        "reasoning": {"type": "effort", "effort": {"values": ["low", "HIGH", "vendor-private", "x".repeat(2_000)]}, "default_effort": "high"},
        "supported_parameters": ["reasoning"]
    }]});
    let catalog = ModelMetadataCatalog::from_metadata_payload(
        &enrich_reasoning_metadata(&models, Some(&openrouter), None),
        None,
        None,
        false,
    )
    .unwrap();
    let capabilities = catalog.capabilities_for("openrouter/provider/model");
    assert_eq!(capabilities.reasoning_method, Some(ReasoningMethod::Effort));
    assert_eq!(capabilities.reasoning_effort_levels, ["low", "high"]);
    assert_eq!(capabilities.default_reasoning_effort, Some("high".into()));
}

#[test]
fn parses_models_dev_reasoning_options() {
    let catalog = catalog(
        r#"{
        "xai/grok-4.6": {
            "name": "Grok 4.6",
            "reasoning": true,
            "reasoning_options": [
                {"type": "effort", "values": ["low", "medium", "high", "xhigh"]}
            ]
        }
    }"#,
    );
    let capabilities = catalog.capabilities_for("x-ai/grok-4.6");
    assert_eq!(capabilities.reasoning, Some(true));
    assert_eq!(
        capabilities.reasoning_effort_levels,
        ["low", "medium", "high", "xhigh"]
    );
}

#[test]
fn keeps_budget_reasoning_separate_from_effort_levels() {
    let catalog = catalog(
        r#"{
        "anthropic/claude-sonnet": {
            "name": "Claude Sonnet",
            "reasoning": true,
            "reasoning_options": [
                {"type": "budget_tokens", "min": 1024, "max": 32000, "default": 8192}
            ]
        }
    }"#,
    );
    let capabilities = catalog.capabilities_for("claude-sonnet");
    assert_eq!(
        capabilities.reasoning_method,
        Some(ReasoningMethod::BudgetTokens)
    );
    assert!(capabilities.reasoning_effort_levels.is_empty());
    assert_eq!(capabilities.reasoning_budget_min_tokens, Some(1024));
    assert_eq!(capabilities.reasoning_budget_max_tokens, Some(32_000));
    assert_eq!(capabilities.reasoning_budget_default_tokens, Some(8_192));
}

#[test]
fn enriches_models_dev_records_from_nested_api_details() {
    let models = serde_json::json!({
        "xai/grok-4.6": {"reasoning": true}
    });
    let details = serde_json::json!({
        "xai": {"models": {
            "grok-4.6": {
                "id": "grok-4.6",
                "reasoning": true,
                "reasoning_options": [{
                    "type": "effort",
                    "values": ["low", "medium", "high", "xhigh"]
                }]
            }
        }}
    });
    let merged = reasoning::enrich_reasoning_metadata_with_models_dev_details(
        models,
        Some(details),
        None,
        None,
    );
    assert_eq!(
        merged["xai/grok-4.6"]["reasoning_effort_levels"],
        serde_json::json!(["low", "medium", "high", "xhigh"])
    );
    assert_eq!(
        merged["xai/grok-4.6"]["reasoning_source"],
        serde_json::json!("models_dev")
    );
}

#[test]
fn preserves_details_reasoning_levels_when_options_describe_another_control() {
    let models = serde_json::json!({
        "vendor/model": {"reasoning": true}
    });
    let details = serde_json::json!({
        "vendor": {"models": {
            "model": {
                "id": "model",
                "reasoning": {
                    "type": "effort",
                    "supported_efforts": ["low", "high"]
                },
                "reasoning_options": [{
                    "type": "toggle",
                    "enabled": true
                }]
            }
        }}
    });

    let merged = reasoning::enrich_reasoning_metadata_with_models_dev_details(
        models,
        Some(details),
        None,
        None,
    );

    assert_eq!(
        merged["vendor/model"]["reasoning_effort_levels"],
        serde_json::json!(["low", "high"])
    );
}

#[test]
fn recognizes_litellm_camel_case_effort_flags_without_openrouter() {
    let models = serde_json::json!({"provider/model": {"reasoning": true}});
    let litellm = serde_json::json!({"provider/model": {
        "supportsLowReasoningEffort": true,
        "supportsHighReasoningEffort": true,
        "supportsVeryHighReasoningEffort": true,
        "supports_very_high_reasoning_effort": true,
        "supports_extra_high_reasoning_effort": true,
        "supportsMaxReasoningEffort": false
    }});
    let catalog = ModelMetadataCatalog::from_metadata_payload(
        &enrich_reasoning_metadata(&models, None, Some(&litellm)),
        None,
        None,
        false,
    )
    .unwrap();
    let capabilities = catalog.capabilities_for("provider/model");
    assert_eq!(capabilities.reasoning_method, Some(ReasoningMethod::Effort));
    assert_eq!(
        capabilities.reasoning_effort_levels,
        ["low", "high", "xhigh"]
    );
}

#[test]
fn matches_decimal_and_dashed_model_versions_before_litellm_fallback() {
    let models = serde_json::json!({
        "openrouter/anthropic/claude-fable-5-1": {"reasoning": true},
        "openrouter/anthropic/claude-opus-4-8": {"reasoning": true}
    });
    let openrouter = serde_json::json!({"data": [
        {
            "id": "anthropic/claude-fable-5.1",
            "reasoning": {"supported_efforts": ["max", "xhigh", "high", "medium", "low"]}
        },
        {
            "id": "anthropic/claude-fable-5.1:batch",
            "reasoning": {"supported_efforts": ["max"]}
        },
        {
            "id": "anthropic/claude-opus-4.8",
            "reasoning": {"supported_efforts": ["max", "xhigh", "high", "medium", "low"]}
        }
    ]});
    let litellm = serde_json::json!({
        "claude-fable-5-1": {
            "supports_xhigh_reasoning_effort": true,
            "supports_max_reasoning_effort": true
        },
        "claude-opus-4-8": {"supports_max_reasoning_effort": true}
    });

    let catalog = ModelMetadataCatalog::from_metadata_payload(
        &enrich_reasoning_metadata(&models, Some(&openrouter), Some(&litellm)),
        None,
        None,
        false,
    )
    .unwrap();

    assert_eq!(
        catalog.reasoning_levels_for("claude-fable-5-1"),
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(
        catalog.reasoning_levels_for("claude-opus-4-8"),
        ["low", "medium", "high", "xhigh", "max"]
    );
    assert_eq!(
        catalog
            .capabilities_for("claude-fable-5-1")
            .reasoning_source
            .as_deref(),
        Some("openrouter")
    );
}
