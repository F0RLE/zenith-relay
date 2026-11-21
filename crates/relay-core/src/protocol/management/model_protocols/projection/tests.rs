use super::*;
use crate::ProtocolFeature;

fn route(client: WireApi, upstream: WireApi, features: serde_json::Value) -> ModelProtocolRoute {
    serde_json::from_value(serde_json::json!({
        "clientWireApi":client, "upstreamWireApi":upstream,
        "features":features, "reasoningEfforts":["high"]
    }))
    .unwrap()
}

#[test]
fn client_projection_combines_only_usable_scoped_alternatives() {
    let routes = [
        route(
            WireApi::Responses,
            WireApi::Responses,
            serde_json::json!({"function_tools":"unsupported"}),
        ),
        route(
            WireApi::Responses,
            WireApi::Messages,
            serde_json::json!({"function_tools":"confirmed","streaming":"unsupported"}),
        ),
        route(
            WireApi::Messages,
            WireApi::Messages,
            serde_json::json!({"function_tools":"confirmed"}),
        ),
        route(
            WireApi::Responses,
            WireApi::GeminiInteractions,
            serde_json::json!({"function_tools":"confirmed"}),
        ),
    ];
    let reference = ModelCapabilities::unknown_model();
    let projected = capabilities_for_client(&routes, WireApi::Responses, &reference).unwrap();
    assert_eq!(projected.tool_call, Some(false));
    assert_eq!(projected.parallel_tool_call, Some(false));
    assert!(capabilities_for_client(&routes, WireApi::Gemini, &reference).is_none());
    assert!(capabilities_for_client(&[], WireApi::Responses, &reference).is_none());
}

#[test]
fn alternative_union_preserves_unknown_and_positive_statuses() {
    let reference = ModelCapabilities::unknown_model();
    let denied = route(
        WireApi::Responses,
        WireApi::Responses,
        serde_json::json!({"function_tools":"unsupported"}),
    );
    let unknown = route(WireApi::Responses, WireApi::Messages, serde_json::json!({}));
    let supported = route(
        WireApi::Responses,
        WireApi::Gemini,
        serde_json::json!({"function_tools":"confirmed"}),
    );
    let projected =
        capabilities_for_client(&[denied.clone(), unknown], WireApi::Responses, &reference)
            .unwrap();
    assert_eq!(projected.tool_call, None);
    let projected =
        capabilities_for_client(&[denied, supported], WireApi::Responses, &reference).unwrap();
    assert_eq!(projected.tool_call, Some(true));
    assert_eq!(projected.parallel_tool_call, None);
    assert_eq!(projected.default_reasoning_effort, None);
}

#[test]
fn resolved_efforts_never_fall_back_after_denial_or_empty_intersection() {
    let reference = ModelCapabilities {
        reasoning: Some(true),
        reasoning_effort_levels: vec!["low".into(), "high".into()],
        ..ModelCapabilities::unknown_model()
    };
    let mut resolved = route(
        WireApi::Responses,
        WireApi::Responses,
        serde_json::json!({"reasoning":"confirmed"}),
    );
    for levels in [vec![], vec!["medium".into()]] {
        resolved.reasoning_efforts = levels;
        let projected = capabilities_for_client(
            std::slice::from_ref(&resolved),
            WireApi::Responses,
            &reference,
        )
        .unwrap();
        assert!(projected.reasoning_effort_levels.is_empty());
        assert_eq!(projected.default_reasoning_effort, None);
    }
    resolved
        .features
        .insert(ProtocolFeature::Reasoning, CapabilityStatus::Unsupported);
    resolved.reasoning_efforts = vec!["high".into()];
    let projected = capabilities_for_client(&[resolved], WireApi::Responses, &reference).unwrap();
    assert_eq!(projected.reasoning, Some(false));
    assert!(projected.reasoning_effort_levels.is_empty());
}
