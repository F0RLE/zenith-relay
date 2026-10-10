use crate::gateway::request::{candidate_protocols, requested_reasoning_effort, ServiceTierPolicy};
use crate::runtime::{AccountTransport, AuthenticatedKey, DefaultServiceTier};
use crate::{
    AdapterError, AdapterRequestContext, CapabilityStatus, GatewayRuntime, ProtocolFeature, WireApi,
};
use serde_json::Value;
use std::collections::HashSet;

/// Admission happens before a member consumes rotation credit or capacity.
pub(super) struct RouteCompatibility<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) model: &'a str,
    pub(super) client: WireApi,
    pub(super) request: &'a Value,
    pub(super) stream: bool,
    pub(super) tier_policy: &'a ServiceTierPolicy,
    pub(super) now_ms: u64,
}

pub(super) fn incompatible_routes(
    input: RouteCompatibility<'_>,
) -> (HashSet<String>, Option<AdapterError>) {
    let RouteCompatibility {
        runtime,
        key,
        model,
        client,
        request,
        stream,
        tier_policy,
        now_ms,
    } = input;
    let mut excluded = HashSet::new();
    let mut last_error = None;
    let features = requested_features(request, stream);
    let effort = requested_reasoning_effort(request, client);
    for route in runtime.configured_executor_routes(key, model, candidate_protocols(client), stream)
    {
        let validate = || {
            if route.account_transport == AccountTransport::ExcelBasisPoints {
                let selected = tier_policy.select_for_model(runtime, &route.source_model);
                if let Some(error) =
                    basis_points_route_error(request, stream, tier_policy, selected)
                {
                    return Err(error);
                }
            }

            // Reference model capabilities and exact source-route evidence
            // constrain conversions. A raw /models listing alone is not
            // capability evidence. Native payloads still pass through.
            if !route.adapter.is_passthrough() {
                let capability = runtime.model_capabilities(&route.source_model);
                let supported =
                    capability.protocol_features_for_route(route.route_capability.as_ref());
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
                    let levels = capability
                        .reasoning_effort_levels_for_route(route.route_capability.as_ref());
                    !levels.is_empty()
                        && !levels
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

            let continuation_state = if route.adapter.uses_local_continuation_state() {
                request
                    .get("previous_response_id")
                    .and_then(Value::as_str)
                    .filter(|response_id| !response_id.trim().is_empty())
                    .map(|response_id| {
                        runtime.load_messages_bridge_state(
                            &key.id,
                            response_id,
                            &route.candidate_id,
                            now_ms,
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            let mut candidate_request = request.clone();
            tier_policy.prepare_for_candidate(
                &mut candidate_request,
                tier_policy.select_for_model(runtime, &route.source_model),
                client,
            );
            route
                .adapter
                .prepare_request(AdapterRequestContext {
                    client_wire_api: client,
                    request: &candidate_request,
                    model: &route.source_model,
                    stream,
                    reasoning_mode: route.reasoning_mode,
                    cache_write_ttl: route.cache_write_ttl,
                    previous: continuation_state,
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

pub(super) fn basis_points_route_error(
    _request: &Value,
    _stream: bool,
    tier_policy: &ServiceTierPolicy,
    selected_tier: DefaultServiceTier,
) -> Option<AdapterError> {
    if tier_policy.rejects_basis_points_client_speed()
        || selected_tier != DefaultServiceTier::Standard
    {
        return Some(AdapterError::parameter_unsupported_for("service_tier"));
    }
    None
}

/// Shared pre-dispatch rejection for a Basis Points account.
///
/// An unsupported account endpoint wins over Responses Lite. Both win over a
/// request the transport cannot carry. Callers still decide whether to skip
/// the candidate or stop the request.
pub(super) fn basis_points_admission_error(
    request: &Value,
    stream: bool,
    tier_policy: &ServiceTierPolicy,
    selected_tier: DefaultServiceTier,
    responses_lite: bool,
    rejected_endpoint: bool,
) -> Option<AdapterError> {
    if rejected_endpoint {
        return Some(AdapterError::parameter_unsupported_for("endpoint"));
    }
    if responses_lite {
        return Some(AdapterError::parameter_unsupported_for("responses_lite"));
    }
    basis_points_route_error(request, stream, tier_policy, selected_tier)
}

fn requested_features(request: &Value, stream: bool) -> Vec<ProtocolFeature> {
    let mut requested_features = vec![ProtocolFeature::Text];
    if stream {
        requested_features.push(ProtocolFeature::Streaming);
    }
    if request
        .get("tools")
        .and_then(Value::as_array)
        .is_some_and(|tools| !tools.is_empty())
        || ["input", "messages", "contents"]
            .iter()
            .any(|field_name| request.get(field_name).is_some_and(has_tool))
    {
        requested_features.push(ProtocolFeature::FunctionTools);
    }
    let present =
        |field_value: Option<&Value>| field_value.is_some_and(|field_value| !field_value.is_null());
    if present(request.get("tool_choice")) || present(request.get("toolConfig")) {
        requested_features.push(ProtocolFeature::ToolChoice);
    }
    let structured = |format_value: Option<&Value>| {
        format_value.is_some_and(|format_value| {
            !format_value.is_null()
                && format_value.get("type").and_then(Value::as_str) != Some("text")
        })
    };
    if structured(request.get("response_format"))
        || structured(request.pointer("/text/format"))
        || structured(request.pointer("/output_config/format"))
        || request
            .pointer("/generationConfig/responseMimeType")
            .is_some_and(|response_mime_type| {
                !response_mime_type.is_null() && response_mime_type != "text/plain"
            })
    {
        requested_features.push(ProtocolFeature::StructuredOutput);
    }
    if present(request.pointer("/reasoning/effort"))
        || present(request.get("reasoning_effort"))
        || present(request.get("thinking"))
        || present(request.pointer("/generationConfig/thinkingConfig"))
    {
        requested_features.push(ProtocolFeature::Reasoning);
    }
    if ["input", "messages", "contents"]
        .iter()
        .any(|field_name| request.get(field_name).is_some_and(has_image))
    {
        requested_features.push(ProtocolFeature::Images);
    }
    requested_features
}

fn has_tool(request_value: &Value) -> bool {
    match request_value {
        Value::Array(request_items) => request_items.iter().any(has_tool),
        Value::Object(request_object) => {
            request_object.contains_key("tool_calls")
                || request_object.contains_key("functionCall")
                || request_object.contains_key("functionResponse")
                || request_object.get("role").and_then(Value::as_str) == Some("tool")
                || matches!(
                    request_object.get("type").and_then(Value::as_str),
                    Some("function_call" | "function_call_output" | "tool_use" | "tool_result")
                )
                || ["content", "parts"]
                    .iter()
                    .any(|field_name| request_object.get(*field_name).is_some_and(has_tool))
        }
        _ => false,
    }
}

fn has_image(request_value: &Value) -> bool {
    match request_value {
        Value::Array(request_items) => request_items.iter().any(has_image),
        Value::Object(request_object) => {
            request_object.contains_key("inlineData")
                || request_object.contains_key("fileData")
                || matches!(
                    request_object.get("type").and_then(Value::as_str),
                    Some("input_image" | "image_url" | "image")
                )
                || ["content", "parts"]
                    .iter()
                    .any(|field_name| request_object.get(*field_name).is_some_and(has_image))
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

    #[test]
    fn basis_points_rejects_unverified_speed_but_keeps_images_and_tool_bridge_eligible() {
        let implicit = ServiceTierPolicy::pool_owned(&json!({}));
        assert!(basis_points_route_error(
            &json!({"input": "hello"}),
            false,
            &implicit,
            DefaultServiceTier::Standard,
        )
        .is_none());
        for tier in ["auto", "default", "standard"] {
            let ordinary = ServiceTierPolicy::pool_owned(&json!({"service_tier": tier}));
            assert!(
                basis_points_route_error(
                    &json!({"input": "hello"}),
                    false,
                    &ordinary,
                    DefaultServiceTier::Standard,
                )
                .is_none(),
                "{tier}"
            );
        }

        let explicit_speed = ServiceTierPolicy::pool_owned(&json!({"service_tier": "priority"}));
        assert_eq!(
            basis_points_route_error(
                &json!({"input": "hello"}),
                false,
                &explicit_speed,
                DefaultServiceTier::Standard,
            )
            .and_then(|error| error.parameter()),
            Some("service_tier")
        );
        assert_eq!(
            basis_points_route_error(
                &json!({"input": "hello"}),
                false,
                &implicit,
                DefaultServiceTier::Fast,
            )
            .and_then(|error| error.parameter()),
            Some("service_tier")
        );
        assert!(basis_points_route_error(
            &json!({
                "input": [{"role": "user", "content": [{"type": "input_image", "image_url": "data:image/png;base64,AA=="}]}]
            }),
            false,
            &implicit,
            DefaultServiceTier::Standard,
        )
        .is_none());
        assert!(basis_points_route_error(
            &json!({"tools": [{"type": "function", "name": "lookup"}]}),
            false,
            &implicit,
            DefaultServiceTier::Standard,
        )
        .is_none());
    }
}
