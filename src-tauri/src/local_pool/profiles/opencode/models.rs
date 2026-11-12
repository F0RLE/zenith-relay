use crate::local_pool::error::LocalPoolError;
use serde_json::{Map, Value, json};
use zenith_relay_core::{
    CapabilityStatus, ModelEndpointCapability, ProtocolFeature, WireApi,
    model_metadata::{ModelCapabilities, ModelMetadataCatalog},
    protocol::{ModelProtocolRoute, ModelSummary},
};

/// Use the canonical management projection consumed by the UI. It already
/// applies pool membership, model rules, hidden models and display order, so
/// integrations cannot drift from Relay's own model catalog.
pub(super) fn model_ids(models: &[ModelSummary]) -> Vec<ModelSummary> {
    models
        .iter()
        .filter(|model| {
            model.enabled
                && model.protocol_routes.iter().any(|route| {
                    super::protocols::supports_sdk(route.client_wire_api)
                        && super::protocols::supports_streaming(route)
                })
        })
        .cloned()
        .collect()
}

pub(super) fn capability_config(
    id: &str,
    capabilities: &ModelCapabilities,
    levels: &[String],
) -> Value {
    let mut capability_json = json!({
        "name": id,
        "attachment": capabilities.attachment.unwrap_or_else(|| capabilities.input_modalities.iter().any(|mode| mode != "text")),
        "reasoning": capabilities.reasoning == Some(true),
        "tool_call": capabilities.tool_call == Some(true),
        "modalities": {"input": capabilities.input_modalities, "output": capabilities.output_modalities}
    });
    if let Some(temperature) = capabilities.temperature {
        capability_json["temperature"] = json!(temperature);
    }
    if let (Some(context_limit), Some(output_limit)) =
        (capabilities.context_limit, capabilities.output_limit)
    {
        capability_json["limit"] =
            json!({"context": context_limit, "output": output_limit.min(context_limit)});
        if let Some(input) = capabilities.input_limit {
            capability_json["limit"]["input"] = json!(input.min(context_limit));
        }
    }
    if capabilities.reasoning == Some(true) && !levels.is_empty() {
        capability_json["variants"] = Value::Object(
            levels
                .iter()
                .map(|level| (level.clone(), json!({"reasoningEffort": level})))
                .collect(),
        );
    }
    if capabilities.attachment == Some(false) {
        restrict_image_input(&mut capability_json);
    }
    capability_json
}

pub(super) fn restrict_capability_config(
    configured: &mut Value,
    unsupported: impl Fn(ProtocolFeature) -> bool,
) {
    for (field, feature) in [
        ("tool_call", ProtocolFeature::FunctionTools),
        ("reasoning", ProtocolFeature::Reasoning),
    ] {
        if unsupported(feature) {
            configured[field] = false.into();
        }
    }
    if unsupported(ProtocolFeature::Images) {
        restrict_image_input(configured);
    }
}

fn restrict_image_input(configured: &mut Value) {
    let Some(input) = configured
        .pointer_mut("/modalities/input")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    input.retain(|mode| {
        !mode
            .as_str()
            .is_some_and(|mode| mode.eq_ignore_ascii_case("image"))
    });
    let text_only = input.iter().all(|mode| {
        mode.as_str()
            .is_some_and(|mode| mode.eq_ignore_ascii_case("text"))
    });
    if text_only {
        configured["attachment"] = false.into();
    }
}

#[cfg(test)]
pub(super) fn model_config(models: &[ModelSummary]) -> Map<String, Value> {
    model_config_for_scope(models, None)
}

pub(super) fn model_config_for_protocol(
    models: &[ModelSummary],
    protocol: WireApi,
) -> Map<String, Value> {
    model_config_for_scope(models, Some(protocol))
}

fn model_config_for_scope(
    models: &[ModelSummary],
    protocol: Option<WireApi>,
) -> Map<String, Value> {
    models
        .iter()
        .filter_map(|model| {
            let mut capabilities = model.catalog_capabilities.clone().unwrap_or_else(|| {
                if model.catalog_input_modalities.is_empty() && model.catalog_provider.is_none() {
                    ModelCapabilities::unknown_model()
                } else {
                    ModelCapabilities {
                        reasoning: model.catalog_reasoning,
                        tool_call: model.catalog_tool_call,
                        attachment: model.catalog_attachment,
                        input_modalities: model.catalog_input_modalities.clone(),
                        output_modalities: model.catalog_output_modalities.clone(),
                        context_limit: model.catalog_context_limit,
                        input_limit: model.catalog_input_limit,
                        output_limit: model.catalog_output_limit,
                        ..ModelCapabilities::default()
                    }
                }
            });
            if model.context_configurable {
                capabilities.context_limit =
                    zenith_relay_core::catalog::context_policy::context_window_tokens(
                        model.context_policy,
                        model
                            .context_windows
                            .and_then(|windows| windows.default_tokens),
                        model
                            .context_windows
                            .map_or(capabilities.context_limit, |windows| windows.maximum_tokens),
                    );
            }
            if let Some(protocol) = protocol {
                capabilities = ModelProtocolRoute::capabilities_for_client(
                    &model.protocol_routes,
                    protocol,
                    &capabilities,
                )?;
            }
            let supported_levels = if protocol.is_some() {
                &capabilities.reasoning_effort_levels
            } else if model.reasoning_supported_levels.is_empty() {
                &model.catalog_reasoning_effort_levels
            } else {
                &model.reasoning_supported_levels
            };
            let levels = supported_levels
                .iter()
                .filter(|level| {
                    !model.reasoning_configurable || model.reasoning_allowed_levels.contains(level)
                })
                .cloned()
                .collect::<Vec<_>>();
            Some((
                model.id.clone(),
                capability_config(&model.id, &capabilities, &levels),
            ))
        })
        .collect()
}

pub(super) fn model_config_ids(
    models: &[String],
    metadata: &ModelMetadataCatalog,
    protocol: WireApi,
    endpoint_capabilities: &[ModelEndpointCapability],
) -> Map<String, Value> {
    models
        .iter()
        .filter(|model| !model.trim().is_empty())
        .filter_map(|model| {
            let endpoint = source_model_endpoint(model, protocol, endpoint_capabilities);
            let capabilities = metadata.capabilities_for(model).for_route(endpoint);
            if capabilities.streaming == Some(false) {
                return None;
            }
            let features = capabilities.protocol_features_for_route(endpoint);
            let mut configured =
                capability_config(model, &capabilities, &capabilities.reasoning_effort_levels);
            restrict_capability_config(&mut configured, |feature| {
                features.get(&feature) == Some(&CapabilityStatus::Unsupported)
            });
            Some((model.clone(), configured))
        })
        .collect()
}

pub(super) fn source_model_endpoint<'a>(
    model: &str,
    protocol: WireApi,
    endpoint_capabilities: &'a [ModelEndpointCapability],
) -> Option<&'a ModelEndpointCapability> {
    let model = zenith_relay_core::model_id_key(model);
    endpoint_capabilities.iter().find(|endpoint| {
        endpoint.upstream_wire_api == protocol
            && zenith_relay_core::model_id_key(&endpoint.model_id) == model
    })
}

#[cfg(test)]
pub(super) fn managed_provider(base_url: &str, secret: &str, models: &[ModelSummary]) -> Value {
    json!({
        "npm": super::PROVIDER_NPM,
        "name": "Zenith Relay",
        "options": {
            "baseURL": base_url,
            "apiKey": secret,
        },
        "models": model_config(models),
    })
}

pub(super) fn apply_managed_provider(
    config: &mut Map<String, Value>,
    base_url: &str,
    secret: &str,
    models: &[ModelSummary],
) -> Result<(), LocalPoolError> {
    super::protocols::apply(config, base_url, secret, models)
}
