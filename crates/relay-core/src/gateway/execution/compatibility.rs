use crate::gateway::request::{candidate_protocols, requested_reasoning_effort, ServiceTierPolicy};
use crate::runtime::{AccountTransport, AuthenticatedKey, DefaultServiceTier, ExecutorRoute};
use crate::{
    AdapterError, AdapterRequestContext, CapabilityStatus, GatewayRuntime, ProtocolFeature, WireApi,
};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Admission happens before a member consumes rotation credit or capacity.
pub(in crate::gateway) struct RouteCompatibility<'a> {
    pub(in crate::gateway) runtime: &'a GatewayRuntime,
    pub(in crate::gateway) key: &'a AuthenticatedKey,
    pub(in crate::gateway) model: &'a str,
    pub(in crate::gateway) client: WireApi,
    pub(in crate::gateway) request: &'a Value,
    pub(in crate::gateway) stream: bool,
    pub(in crate::gateway) tier_policy: &'a ServiceTierPolicy,
    pub(in crate::gateway) now_ms: u64,
}

pub(in crate::gateway) fn incompatible_routes(
    input: RouteCompatibility<'_>,
) -> (
    HashSet<String>,
    BTreeMap<String, ExecutorRoute>,
    Option<AdapterError>,
) {
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
    let mut admitted = BTreeMap::new();
    let mut last_error = None;
    let features = requested_features(request, stream);
    let effort = requested_reasoning_effort(request, client);
    // A picker is not an enforcement boundary. Check the same model policy
    // for every client, including Desktop's single opaque model entry.
    let effort_denied_by_pool = effort.as_ref().is_some_and(|effort| {
        runtime
            .model_reasoning_policy_levels(model)
            .is_some_and(|levels| {
                !levels
                    .iter()
                    .any(|level| level.eq_ignore_ascii_case(effort))
            })
    });
    for route in runtime.configured_executor_routes(key, model, candidate_protocols(client), stream)
    {
        if admitted.contains_key(&route.candidate_id) {
            continue;
        }
        let validate = || {
            if effort_denied_by_pool {
                return Err(AdapterError::reasoning_unsupported());
            }
            runtime.validate_continuation_route(&route, &key.id, request, now_ms)?;
            let capability = runtime
                .model_capabilities(&route.source_model)
                .for_route(route.route_capability.as_ref());
            let selected = tier_policy.selected_tier();
            if !GatewayRuntime::route_supports_service_tier(&route, selected) {
                return Err(AdapterError::parameter_unsupported_for("service_tier"));
            }
            if route.account_transport == AccountTransport::ExcelBasisPoints {
                if let Some(error) =
                    basis_points_route_error(request, stream, tier_policy, selected)
                {
                    return Err(error);
                }
            }

            // Reference model capabilities and exact source-route evidence
            // constrain conversions. A raw /models listing alone is not
            // capability evidence. Native payloads still pass through.
            {
                if capability.temperature == Some(false)
                    && request
                        .pointer(if client == WireApi::Gemini {
                            "/generationConfig/temperature"
                        } else {
                            "/temperature"
                        })
                        .is_some_and(|value| !value.is_null())
                {
                    return Err(AdapterError::parameter_unsupported_for("temperature"));
                }
                // Allowing parallel calls does not require the model to
                // produce them. The adapter owns representation of this
                // permission and enforcement of an explicit single-call limit.
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
                    let levels = &capability.reasoning_effort_levels;
                    !(effort == "none" && capability.reasoning == Some(false))
                        && !levels.is_empty()
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

            let continuation_state =
                runtime.load_adapter_continuation(&route, &key.id, request, now_ms)?;
            let mut candidate_request = request.clone();
            tier_policy.prepare_for_candidate(
                &mut candidate_request,
                tier_policy.selected_tier(),
                client,
            );
            let prepared = route.adapter.prepare_request_with_capabilities(
                AdapterRequestContext {
                    client_wire_api: client,
                    request: &candidate_request,
                    model: &route.source_model,
                    stream,
                    reasoning_mode: route.reasoning_mode,
                    cache_write_ttl: route.cache_write_ttl,
                    previous: continuation_state,
                    response_scope: &route.candidate_id,
                    response_id_seed: "admission",
                },
                &capability,
            )?;
            if route.account_transport == AccountTransport::ExcelBasisPoints {
                if let Some(error) =
                    super::basis_points::output_format_error(prepared.upstream_body())
                {
                    return Err(error);
                }
            }
            Ok(())
        };
        match validate() {
            Err(error) => {
                excluded.insert(route.candidate_id);
                last_error = Some(error);
            }
            Ok(()) => {
                excluded.remove(&route.candidate_id);
                admitted.insert(route.candidate_id.clone(), route);
            }
        }
    }
    (excluded, admitted, last_error)
}

pub(in crate::gateway) struct AdapterRejection<'a> {
    pub request_id: &'a str,
    pub attempt: u16,
    pub local_key_id: &'a str,
    pub requested_model: &'a str,
    pub resolved_model: &'a str,
    pub client_context_id: Option<&'a str>,
    pub wire_api: WireApi,
    pub transport: crate::UsageTransport,
    pub service_tier: DefaultServiceTier,
}

/// Preparation can fail before a route is reserved. Record the logical request
/// without inventing a provider attempt, tokens, or a health/quota failure.
pub(in crate::gateway) fn record_adapter_rejection(
    runtime: &GatewayRuntime,
    request: AdapterRejection<'_>,
    error: &AdapterError,
    latency_ms: u64,
) {
    let event = crate::UsageEvent {
        request_id: request.request_id.to_owned(),
        attempt: request.attempt,
        local_key_id: request.local_key_id.to_owned(),
        source_id: "relay".to_owned(),
        candidate_id: None,
        account_id: None,
        account_token_generation: None,
        client_context_id: request.client_context_id.map(str::to_owned),
        routing: None,
        requested_model: Some(request.requested_model.to_owned()),
        resolved_model: Some(request.resolved_model.to_owned()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: request.wire_api,
        transport: request.transport,
        service_tier: request.service_tier,
        applied_service_tier: None,
        success: false,
        http_status: if error.is_upstream_failure() {
            502
        } else {
            400
        },
        error_category: Some(error.code().to_owned()),
        tool_use: Default::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
        upstream_error: None,
        quota_snapshot: None,
        image_usage: None,
    };
    crate::gateway::response::emit_usage(runtime, event);
}

pub(super) fn basis_points_route_error(
    request: &Value,
    _stream: bool,
    tier_policy: &ServiceTierPolicy,
    selected_tier: DefaultServiceTier,
) -> Option<AdapterError> {
    if tier_policy.rejects_basis_points_client_speed()
        || selected_tier != DefaultServiceTier::Standard
    {
        return Some(AdapterError::parameter_unsupported_for("service_tier"));
    }
    super::basis_points::output_format_error(request)
}

/// Shared pre-dispatch rejection for a Basis Points account.
///
/// An unsupported account endpoint wins over a request the transport cannot
/// carry. The Basis Points adapter translates the Responses Lite item-level
/// tool catalog locally; callers keep that body normalization but do not
/// forward the Lite transport header. Callers still decide whether to skip
/// the candidate or stop the request.
pub(super) fn basis_points_admission_error(
    request: &Value,
    stream: bool,
    tier_policy: &ServiceTierPolicy,
    selected_tier: DefaultServiceTier,
    rejected_endpoint: bool,
) -> Option<AdapterError> {
    if rejected_endpoint {
        return Some(AdapterError::parameter_unsupported_for("endpoint"));
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
    let active_effort = |value: Option<&Value>| {
        value.is_some_and(|value| {
            !value.is_null()
                && !value.as_str().is_some_and(|effort| {
                    crate::catalog::canonical_reasoning_level(effort) == "none"
                })
        })
    };
    if active_effort(request.pointer("/reasoning/effort"))
        || active_effort(request.get("reasoning_effort"))
        || request.get("thinking").is_some_and(|thinking| {
            !thinking.is_null() && thinking.get("type").and_then(Value::as_str) != Some("disabled")
        })
        || request
            .pointer("/generationConfig/thinkingConfig")
            .is_some_and(|thinking| {
                !thinking.is_null()
                    && thinking.get("thinkingBudget").and_then(Value::as_u64) != Some(0)
            })
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
            ["inlineData", "fileData"].iter().any(|field| {
                request_object
                    .get(*field)
                    .and_then(|media| media.get("mimeType"))
                    .and_then(Value::as_str)
                    .is_some_and(|mime| mime.starts_with("image/"))
            }) || matches!(
                request_object.get("type").and_then(Value::as_str),
                Some("input_image" | "image_url" | "image")
            ) || ["content", "parts"]
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
    fn gemini_temperature_obeys_model_capabilities_before_route_admission() {
        use crate::model_metadata::{ModelMetadataCatalog, ModelMetadataCatalogHandle};
        use crate::runtime::{GatewayRuntimeOptions, RuntimeLocalKey, RuntimeSource};
        use crate::{LocalGatewayKey, ProviderSource};

        for supported in [false, true] {
            let catalog = ModelMetadataCatalog::from_models_dev_json(
                &json!({"anthropic/temperature-test":{
                    "name":"Temperature Test","family":"claude","temperature":supported
                }})
                .to_string(),
            )
            .unwrap();
            let runtime = GatewayRuntime::from_pool(
                vec![RuntimeSource::unrestricted(ProviderSource {
                    id: "synthetic-source".into(),
                    name: "Synthetic source".into(),
                    base_url: "https://example.test/v1".into(),
                    api_key: "synthetic-upstream-key".into(),
                    proxy_url: None,
                    wire_api: WireApi::Messages,
                    models: vec!["temperature-test".into()],
                })],
                vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                    id: "synthetic-key".into(),
                    secret: "synthetic-secret".into(),
                })],
                GatewayRuntimeOptions {
                    model_metadata_catalog: Some(ModelMetadataCatalogHandle::new(catalog)),
                    ..Default::default()
                },
                std::sync::Arc::new(|_| {}),
            )
            .unwrap();
            let key = runtime.authenticate_secret("synthetic-secret").unwrap();
            for temperature in [None, Some(Value::Null), Some(json!(0.3))] {
                let mut request =
                    json!({"contents":[{"role":"user","parts":[{"text":"Synthetic input"}]}]});
                if let Some(value) = &temperature {
                    request["generationConfig"] = json!({"temperature":value});
                }
                let tier_policy = ServiceTierPolicy::pool_owned(&request);
                let (excluded, admitted, error) = incompatible_routes(RouteCompatibility {
                    runtime: &runtime,
                    key: &key,
                    model: "temperature-test",
                    client: WireApi::Gemini,
                    request: &request,
                    stream: false,
                    tier_policy: &tier_policy,
                    now_ms: 1,
                });
                let rejected = !supported && temperature.is_some_and(|value| !value.is_null());
                assert_eq!(admitted.len(), usize::from(!rejected));
                assert_eq!(excluded.len(), usize::from(rejected));
                assert_eq!(
                    error.and_then(AdapterError::parameter),
                    rejected.then_some("temperature")
                );
            }
        }
    }

    #[test]
    fn neutral_controls_do_not_require_unsupported_features() {
        for request in [
            json!({"reasoning":null,"tool_choice":null,"text":{"format":{"type":"text"}}}),
            json!({"reasoning":{},"response_format":{"type":"text"}}),
            json!({"generationConfig":{"responseMimeType":"text/plain","thinkingConfig":null}}),
            json!({"reasoning":{"effort":"none"}}),
            json!({"reasoning_effort":"NONE"}),
            json!({"thinking":{"type":"disabled"}}),
            json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":0}}}),
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
    fn basis_points_output_format_is_checked_before_admission() {
        let tier = ServiceTierPolicy::pool_owned(&json!({}));
        let mut request = json!({"text":{"format":{
            "type":"json_schema","name":"command_review","strict":false,
            "schema":{"type":"object","properties":{"approved":{"type":"boolean"}},
                "required":["approved"],"additionalProperties":false}
        }}});
        for stream in [false, true] {
            assert!(basis_points_route_error(
                &request,
                stream,
                &tier,
                DefaultServiceTier::Standard
            )
            .is_none());
        }
        request["relay_bps_output_validation"] = json!(false);
        assert_eq!(
            basis_points_route_error(&request, false, &tier, DefaultServiceTier::Standard)
                .and_then(|error| error.parameter()),
            Some("text.format")
        );
        request["relay_bps_output_validation"] = json!(true);
        assert!(
            basis_points_route_error(&request, false, &tier, DefaultServiceTier::Standard)
                .is_none()
        );
        request["text"]["format"]["strict"] = json!(true);
        assert_eq!(
            basis_points_route_error(&request, false, &tier, DefaultServiceTier::Standard)
                .and_then(|error| error.parameter()),
            Some("text.format.strict")
        );
        assert!(basis_points_route_error(
            &json!({"text":{"format":{"type":"text"}}}),
            false,
            &tier,
            DefaultServiceTier::Standard,
        )
        .is_none());
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
