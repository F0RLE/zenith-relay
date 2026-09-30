use crate::local_pool::error::LocalPoolError;
use serde_json::{json, Map, Value};
use zenith_relay_core::{
    model_metadata::{ModelCapabilities, ModelMetadataCatalog},
    protocol::ModelSummary,
};

/// Use the canonical management projection consumed by the UI. It already
/// applies pool membership, model rules, hidden models and display order, so
/// integrations cannot drift from Relay's own model catalog.
pub(super) fn model_ids(models: &[ModelSummary]) -> Vec<ModelSummary> {
    models
        .iter()
        .filter(|model| model.enabled && !model.protocol_routes.is_empty())
        .cloned()
        .collect()
}

pub(super) fn capability_config(
    id: &str,
    capabilities: &ModelCapabilities,
    levels: &[String],
) -> Value {
    let mut value = json!({
        "name": id,
        "attachment": capabilities.attachment.unwrap_or_else(|| capabilities.input_modalities.iter().any(|mode| mode != "text")),
        "reasoning": capabilities.reasoning == Some(true),
        "tool_call": capabilities.tool_call == Some(true),
        "modalities": {"input": capabilities.input_modalities, "output": capabilities.output_modalities}
    });
    if let (Some(context), Some(output)) = (capabilities.context_limit, capabilities.output_limit) {
        value["limit"] = json!({"context": context, "output": output});
        if let Some(input) = capabilities.input_limit {
            value["limit"]["input"] = json!(input);
        }
    }
    if capabilities.reasoning == Some(true) && !levels.is_empty() {
        value["variants"] = Value::Object(
            levels
                .iter()
                .map(|level| (level.clone(), json!({"reasoningEffort": level})))
                .collect(),
        );
    }
    value
}

pub(super) fn model_config(models: &[ModelSummary]) -> Map<String, Value> {
    models
        .iter()
        .map(|model| {
            let capabilities =
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
                };
            let supported_levels = if model.reasoning_supported_levels.is_empty() {
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
            (
                model.id.clone(),
                capability_config(&model.id, &capabilities, &levels),
            )
        })
        .collect()
}

pub(super) fn model_config_ids(
    models: &[String],
    metadata: &ModelMetadataCatalog,
) -> Map<String, Value> {
    models
        .iter()
        .filter(|model| !model.trim().is_empty())
        .map(|model| {
            let capabilities = metadata.capabilities_for(model);
            (
                model.clone(),
                capability_config(model, &capabilities, &capabilities.reasoning_effort_levels),
            )
        })
        .collect()
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
