use crate::gateway::request::{candidate_protocols, requested_reasoning_effort, ServiceTierPolicy};
use crate::runtime::AuthenticatedKey;
use crate::{
    AdapterError, AdapterRequestContext, CapabilityStatus, GatewayRuntime, ProtocolFeature, WireApi,
};
use serde_json::Value;
use std::collections::HashSet;

/// Admission happens before a member consumes rotation credit or capacity.
#[allow(clippy::too_many_arguments)]
pub(super) fn incompatible_routes(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    model: &str,
    client: WireApi,
    request: &Value,
    stream: bool,
    tier_policy: &ServiceTierPolicy,
    now_ms: u64,
) -> (HashSet<String>, Option<AdapterError>) {
    let mut excluded = HashSet::new();
    let mut last_error = None;
    let features = requested_features(request, stream);
    let effort = requested_reasoning_effort(request, client);
    for route in runtime.configured_executor_routes(key, model, candidate_protocols(client), stream)
    {
        let validate = || {
            if let Some(capability) =
                runtime.route_capabilities(&route.candidate_id, &route.source_model)
            {
                if features.iter().any(|feature| {
                    capability.features.get(feature) == Some(&CapabilityStatus::Unsupported)
                }) {
                    return Err(AdapterError::parameter_unsupported());
                }
                if effort.as_ref().is_some_and(|effort| {
                    !capability.reasoning_efforts.is_empty()
                        && !capability
                            .reasoning_efforts
                            .iter()
                            .any(|level| level.eq_ignore_ascii_case(effort))
                }) {
                    return Err(AdapterError::reasoning_unsupported());
                }
            }
            let previous = if route.adapter.uses_local_continuation_state() {
                request
                    .get("previous_response_id")
                    .and_then(Value::as_str)
                    .filter(|id| !id.trim().is_empty())
                    .map(|id| {
                        runtime.load_messages_bridge_state(&key.id, id, &route.candidate_id, now_ms)
                    })
                    .transpose()?
            } else {
                None
            };
            let mut body = request.clone();
            tier_policy.prepare_for_candidate(
                &mut body,
                runtime.model_service_tier_for_candidate(&route.candidate_id, &route.source_model),
                client,
            );
            route
                .adapter
                .prepare_request(AdapterRequestContext {
                    client_wire_api: client,
                    request: &body,
                    model: &route.source_model,
                    stream,
                    reasoning_mode: route.reasoning_mode,
                    cache_write_ttl: route.cache_write_ttl,
                    previous,
                    response_scope: &route.candidate_id,
                    response_id_seed: "admission",
                })
                .map(drop)
        };
        if let Err(error) = validate() {
            excluded.insert(route.candidate_id);
            last_error = Some(error);
        }
    }
    (excluded, last_error)
}

fn requested_features(request: &Value, stream: bool) -> Vec<ProtocolFeature> {
    let mut result = vec![ProtocolFeature::Text];
    if stream {
        result.push(ProtocolFeature::Streaming);
    }
    if request
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
        || ["input", "messages", "contents"]
            .iter()
            .any(|key| request.get(key).is_some_and(has_tool))
    {
        result.push(ProtocolFeature::FunctionTools);
    }
    if request.get("tool_choice").is_some() || request.get("toolConfig").is_some() {
        result.push(ProtocolFeature::ToolChoice);
    }
    if request.get("response_format").is_some()
        || request.pointer("/text/format").is_some()
        || request.pointer("/output_config/format").is_some()
        || request
            .pointer("/generationConfig/responseMimeType")
            .is_some()
    {
        result.push(ProtocolFeature::StructuredOutput);
    }
    if request.get("reasoning").is_some()
        || request.get("reasoning_effort").is_some()
        || request.get("thinking").is_some()
        || request
            .pointer("/generationConfig/thinkingConfig")
            .is_some()
    {
        result.push(ProtocolFeature::Reasoning);
    }
    if ["input", "messages", "contents"]
        .iter()
        .any(|key| request.get(key).is_some_and(has_image))
    {
        result.push(ProtocolFeature::Images);
    }
    result
}

fn has_tool(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(has_tool),
        Value::Object(object) => {
            object.contains_key("tool_calls")
                || object.contains_key("functionCall")
                || object.contains_key("functionResponse")
                || object.get("role").and_then(Value::as_str) == Some("tool")
                || matches!(
                    object.get("type").and_then(Value::as_str),
                    Some("function_call" | "function_call_output" | "tool_use" | "tool_result")
                )
                || ["content", "parts"]
                    .iter()
                    .any(|key| object.get(*key).is_some_and(has_tool))
        }
        _ => false,
    }
}

fn has_image(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(has_image),
        Value::Object(object) => {
            object.contains_key("inlineData")
                || object.contains_key("fileData")
                || matches!(
                    object.get("type").and_then(Value::as_str),
                    Some("input_image" | "image_url" | "image")
                )
                || ["content", "parts"]
                    .iter()
                    .any(|key| object.get(*key).is_some_and(has_image))
        }
        _ => false,
    }
}
