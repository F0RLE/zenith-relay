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
            // Reference model capabilities constrain conversions, never the
            // participant's optional /models fields. Native payloads pass through.
            if !route.adapter.is_passthrough() {
                let capability = runtime.model_capabilities(&route.source_model);
                let supported = capability.protocol_features();
                if let Some(feature) = features
                    .iter()
                    .find(|feature| supported.get(feature) == Some(&CapabilityStatus::Unsupported))
                {
                    return Err(AdapterError::parameter_unsupported_for(match feature {
                        ProtocolFeature::Streaming => "stream",
                        ProtocolFeature::FunctionTools => "tools",
                        ProtocolFeature::ToolChoice => "tool_choice",
                        ProtocolFeature::StructuredOutput => "response_format",
                        ProtocolFeature::Reasoning => "reasoning",
                        ProtocolFeature::Images => "input.image",
                        _ => "input",
                    }));
                }
                if effort.as_ref().is_some_and(|effort| {
                    !capability.reasoning_effort_levels.is_empty()
                        && !capability
                            .reasoning_effort_levels
                            .iter()
                            .any(|level| level.eq_ignore_ascii_case(effort))
                }) {
                    return Err(AdapterError::reasoning_unsupported());
                }
            }

            // Native routes for these protocols do not inspect or rewrite the
            // request during preparation. Avoid cloning a large request for
            // every candidate; execution still prepares it after reservation.
            // Messages must validate its configured cache TTL before admission.
            if route.adapter.is_passthrough() && client != WireApi::Messages {
                return route
                    .adapter
                    .validate(client, route.reasoning_mode)
                    .map(drop);
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
                tier_policy.select_for_model(runtime, &route.source_model),
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
    let present = |value: Option<&Value>| value.is_some_and(|v| !v.is_null());
    if present(request.get("tool_choice")) || present(request.get("toolConfig")) {
        result.push(ProtocolFeature::ToolChoice);
    }
    let structured = |value: Option<&Value>| {
        value.is_some_and(|v| !v.is_null() && v.get("type").and_then(Value::as_str) != Some("text"))
    };
    if structured(request.get("response_format"))
        || structured(request.pointer("/text/format"))
        || structured(request.pointer("/output_config/format"))
        || request
            .pointer("/generationConfig/responseMimeType")
            .is_some_and(|v| !v.is_null() && v != "text/plain")
    {
        result.push(ProtocolFeature::StructuredOutput);
    }
    if present(request.pointer("/reasoning/effort"))
        || present(request.get("reasoning_effort"))
        || present(request.get("thinking"))
        || present(request.pointer("/generationConfig/thinkingConfig"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn neutral_controls_do_not_require_unsupported_features() {
        for request in [
            json!({"reasoning":null,"tool_choice":null,"text":{"format":{"type":"text"}}}),
            json!({"reasoning":{},"response_format":{"type":"text"}}),
            json!({"generationConfig":{"responseMimeType":"text/plain","thinkingConfig":null}}),
        ] {
            assert_eq!(requested_features(&request, false), [ProtocolFeature::Text]);
        }
        let features = requested_features(
            &json!({
                "text":{"format":{"type":"json_schema","schema":{}}},
                "reasoning":{"effort":"high"}
            }),
            true,
        );
        assert!(features.contains(&ProtocolFeature::StructuredOutput));
        assert!(features.contains(&ProtocolFeature::Reasoning));
        assert!(features.contains(&ProtocolFeature::Streaming));
    }
}
