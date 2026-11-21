use super::*;
use crate::{CapabilityStatus, ModelEndpointCapability, ProtocolFeature};
use serde_json::json;

fn endpoint(features: Value) -> ModelEndpointCapability {
    serde_json::from_value(json!({
        "modelId":"synthetic-model","upstreamWireApi":"responses",
        "status":"declared","origin":"catalog","checkedAtMs":1,"features":features
    }))
    .unwrap()
}

#[test]
fn selected_route_denials_reach_the_typed_capabilities_without_mutating_the_reference() {
    let reference = ModelCapabilities {
        tool_call: Some(true),
        parallel_tool_call: Some(true),
        tool_choice: Some(true),
        structured_output: Some(true),
        streaming: Some(true),
        reasoning: Some(true),
        reasoning_effort_levels: vec!["low".into(), "high".into()],
        ..Default::default()
    };
    let original = reference.clone();
    let route = endpoint(json!({
        "function_tools":"unsupported","tool_choice":"unsupported",
        "structured_output":"unsupported","streaming":"unsupported","reasoning":"unsupported"
    }));
    let effective = reference.for_route(Some(&route));
    assert_eq!(effective.tool_call, Some(false));
    assert_eq!(effective.parallel_tool_call, Some(false));
    assert_eq!(effective.tool_choice, Some(false));
    assert_eq!(effective.structured_output, Some(false));
    assert_eq!(effective.streaming, Some(false));
    assert_eq!(effective.reasoning, Some(false));
    assert!(effective.reasoning_effort_levels.is_empty());
    assert_eq!(reference, original);
    assert_eq!(effective.for_route(Some(&route)), effective);
}

#[test]
fn exact_route_evidence_fills_unknown_controls_without_inventing_a_default_effort() {
    let reference = ModelCapabilities::unknown_model();
    let unknown = reference.for_route(Some(&endpoint(json!({}))));
    assert_eq!(unknown.tool_call, None);
    assert_eq!(unknown.streaming, None);
    assert_eq!(unknown.reasoning, None);
    let known = reference.for_route(Some(&endpoint(json!({
        "function_tools":"confirmed","streaming":"declared","reasoning":"declared"
    }))));
    assert_eq!(known.tool_call, Some(true));
    assert_eq!(known.streaming, Some(true));
    assert_eq!(known.reasoning, Some(true));
    assert!(known.reasoning_effort_levels.is_empty());
    assert_eq!(known.default_reasoning_effort, None);

    let mut route = endpoint(json!({}));
    route.reasoning_efforts = vec!["high".into()];
    let effort = reference.for_route(Some(&route));
    assert_eq!(effort.reasoning, Some(true));
    assert_eq!(effort.reasoning_effort_levels, ["high"]);
    assert_eq!(effort.default_reasoning_effort, None);
    assert_eq!(reference.reasoning, None);
}

#[test]
fn route_declarations_do_not_revive_explicit_reference_exclusions() {
    let reference = ModelCapabilities {
        tool_call: Some(false),
        tool_choice: Some(false),
        structured_output: Some(false),
        streaming: Some(false),
        reasoning: Some(false),
        ..Default::default()
    };
    let mut route = endpoint(json!({
        "function_tools":"confirmed","tool_choice":"declared",
        "structured_output":"confirmed","streaming":"declared","reasoning":"declared"
    }));
    route.reasoning_efforts = vec!["high".into()];
    let effective = reference.for_route(Some(&route));
    for feature in [
        ProtocolFeature::FunctionTools,
        ProtocolFeature::ToolChoice,
        ProtocolFeature::StructuredOutput,
        ProtocolFeature::Streaming,
        ProtocolFeature::Reasoning,
    ] {
        assert_eq!(
            effective.protocol_features()[&feature],
            CapabilityStatus::Unsupported
        );
    }
    assert!(effective.reasoning_effort_levels.is_empty());
}
