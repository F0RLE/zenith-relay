use super::*;
use zenith_relay_core::{CapabilityStatus, ProtocolFeature};

fn positive_source() -> SourceSummary {
    let mut source = source();
    for endpoint in &mut source.protocol_config.capabilities {
        endpoint.features = BTreeMap::from([
            (ProtocolFeature::Streaming, CapabilityStatus::Declared),
            (ProtocolFeature::FunctionTools, CapabilityStatus::Confirmed),
            (ProtocolFeature::Reasoning, CapabilityStatus::Confirmed),
        ]);
        endpoint.reasoning_efforts = vec!["low".into(), "high".into()];
    }
    source
}

fn configured_model(models: &[ModelSummary]) -> Value {
    let mut config = Map::new();
    apply(
        &mut config,
        "http://127.0.0.1:14998/v1",
        "synthetic",
        models,
    )
    .unwrap();
    config["provider"][PROVIDER_ID]["models"]["opaque-model"].clone()
}

#[test]
fn scoped_positive_controls_fill_unknown_reference_without_changing_model_rules() {
    let metadata =
        ModelMetadataCatalog::from_models_dev_json(r#"{"test/opaque-model":{}}"#).unwrap();
    let models = projected_models(&[positive_source()], &metadata, &BTreeMap::new());
    let before = models.clone();
    let configured = configured_model(&models);
    assert_eq!(configured["tool_call"], true);
    assert_eq!(configured["reasoning"], true);
    for level in ["low", "high"] {
        assert_eq!(configured["variants"][level]["reasoningEffort"], level);
    }
    assert_eq!(models, before);
    assert_eq!(metadata.capabilities_for("opaque-model").tool_call, None);
    assert_eq!(metadata.capabilities_for("opaque-model").reasoning, None);
}

#[test]
fn source_only_efforts_obey_raw_pool_policy_even_when_model_rules_have_no_enum() {
    let metadata =
        ModelMetadataCatalog::from_models_dev_json(r#"{"test/opaque-model":{}}"#).unwrap();
    let sources = [positive_source()];
    for allowed in [vec!["HIGH".into()], vec![], vec!["medium".into()]] {
        let policies = BTreeMap::from([("opaque-model".into(), allowed.clone())]);
        let models = projected_models(&sources, &metadata, &policies);
        let before = models.clone();
        let summary = models
            .iter()
            .find(|model| model.id == "opaque-model")
            .unwrap();
        assert!(!summary.reasoning_configurable);
        assert!(summary.reasoning_supported_levels.is_empty());
        assert!(summary.reasoning_allowed_levels.is_empty());
        let configured = configured_model(&models);
        assert_eq!(configured["reasoning"], true);
        assert_eq!(configured["tool_call"], true);
        assert_eq!(configured["variants"]["low"]["disabled"], true);
        if allowed == ["HIGH"] {
            assert_eq!(configured["variants"]["high"]["reasoningEffort"], "high");
        } else {
            assert_eq!(configured["variants"]["high"]["disabled"], true);
        }
        assert_eq!(models, before);
    }
}

#[test]
fn positive_endpoint_evidence_cannot_revive_reference_denials() {
    let metadata = ModelMetadataCatalog::from_models_dev_json(
        r#"{"test/opaque-model":{"tool_call":false,"reasoning":false}}"#,
    )
    .unwrap();
    let models = projected_models(&[positive_source()], &metadata, &BTreeMap::new());
    let configured = configured_model(&models);
    assert_eq!(configured["tool_call"], false);
    assert_eq!(configured["reasoning"], false);
    for level in ["low", "high"] {
        assert_eq!(configured["variants"][level]["disabled"], true);
    }
}

#[test]
fn reasoning_without_endpoint_enum_retains_default_without_inventing_variants() {
    let metadata =
        ModelMetadataCatalog::from_models_dev_json(r#"{"test/opaque-model":{}}"#).unwrap();
    let mut source = positive_source();
    for endpoint in &mut source.protocol_config.capabilities {
        endpoint.reasoning_efforts.clear();
    }
    let models = projected_models(&[source], &metadata, &BTreeMap::new());
    let configured = configured_model(&models);
    assert_eq!(configured["reasoning"], true);
    for level in ["low", "medium", "high"] {
        assert_eq!(configured["variants"][level]["disabled"], true);
        assert!(
            configured["variants"][level]
                .get("reasoningEffort")
                .is_none()
        );
    }
}

#[test]
fn disjoint_endpoint_efforts_do_not_restore_reference_variants() {
    let metadata = metadata();
    let mut source = positive_source();
    for endpoint in &mut source.protocol_config.capabilities {
        endpoint.reasoning_efforts = vec!["medium".into()];
    }
    let models = projected_models(&[source], &metadata, &BTreeMap::new());
    let configured = configured_model(&models);
    assert_eq!(configured["reasoning"], true);
    for level in ["low", "medium", "high"] {
        assert_eq!(configured["variants"][level]["disabled"], true);
    }
}
