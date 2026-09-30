use super::super::*;
use crate::model_metadata::reasoning::enrich_reasoning_metadata;

fn catalog(raw: &str) -> ModelMetadataCatalog {
    ModelMetadataCatalog::from_models_dev_json(raw).unwrap()
}

#[test]
fn merges_openrouter_reasoning_levels_over_litellm_and_models_dev() {
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
        merged["openai/gpt-test"]["reasoning_effort_levels"],
        serde_json::json!(["low", "high"])
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
fn does_not_invent_levels_from_reasoning_boolean() {
    let models = serde_json::json!({"openai/gpt-test": {"reasoning": true}});
    let merged = enrich_reasoning_metadata(&models, None, None);
    assert!(merged["openai/gpt-test"]
        .get("reasoning_effort_levels")
        .is_none());
}

#[test]
fn parses_reasoning_method_and_filters_unrecognized_efforts() {
    let models = serde_json::json!({"provider/model": {"reasoning": true}});
    let openrouter = serde_json::json!({"data": [{
        "id": "provider/model",
        "reasoning": {"type": "effort", "effort": {"values": ["low", "HIGH", "vendor-private", "x".repeat(2_000)]}, "default_effort": "high"},
        "supported_parameters": ["reasoning"]
    }]});
    let catalog = ModelMetadataCatalog::from_payload(
        &enrich_reasoning_metadata(&models, Some(&openrouter), None),
        None,
        None,
        false,
    )
    .unwrap();
    let capabilities = catalog.capabilities_for("provider/model");
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
        &models,
        Some(&details),
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
        &models,
        Some(&details),
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
        "supportsHighReasoningEffort": true
    }});
    let catalog = ModelMetadataCatalog::from_payload(
        &enrich_reasoning_metadata(&models, None, Some(&litellm)),
        None,
        None,
        false,
    )
    .unwrap();
    let capabilities = catalog.capabilities_for("provider/model");
    assert_eq!(capabilities.reasoning_method, Some(ReasoningMethod::Effort));
    assert_eq!(capabilities.reasoning_effort_levels, ["low", "high"]);
}

#[test]
fn matches_decimal_and_dashed_model_versions_before_litellm_fallback() {
    let models = serde_json::json!({
        "anthropic/claude-fable-5-1": {"reasoning": true},
        "anthropic/claude-opus-4-8": {"reasoning": true}
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

    let catalog = ModelMetadataCatalog::from_payload(
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
