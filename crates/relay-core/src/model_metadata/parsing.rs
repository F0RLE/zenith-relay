use super::{
    order, ModelCapabilities, ModelMetadata, ModelMetadataError, ReasoningMethod, MAX_RECORDS,
    MAX_STRING_LENGTH,
};
use crate::model_metadata::reasoning::{
    normalize_external_levels, parse_effort_flags, parse_reasoning_object, parse_reasoning_options,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub(super) fn parse_metadata(
    source_model_id: &str,
    provider: &str,
    metadata_value: &Value,
) -> Option<ModelMetadata> {
    let metadata_object = metadata_value.as_object()?;
    let string = |key: &str| {
        metadata_object
            .get(key)
            .and_then(Value::as_str)
            .filter(|field_value| {
                !field_value.trim().is_empty() && field_value.len() <= MAX_STRING_LENGTH
            })
            .map(str::to_string)
    };
    let boolean = |key: &str| metadata_object.get(key).and_then(Value::as_bool);
    let modalities = metadata_object.get("modalities").and_then(Value::as_object);
    let architecture = metadata_object
        .get("architecture")
        .and_then(Value::as_object);
    let limits = metadata_object.get("limit").and_then(Value::as_object);
    let supported_parameters = string_array(metadata_object.get("supported_parameters"));
    let reasoning_options = parse_reasoning_options(
        metadata_object
            .get("reasoning_options")
            .or_else(|| metadata_object.get("reasoningOptions")),
    );
    let reasoning_method = metadata_object
        .get("reasoning_method")
        .and_then(Value::as_str)
        .and_then(|method| match method {
            "effort" => Some(ReasoningMethod::Effort),
            "toggle" => Some(ReasoningMethod::Toggle),
            "budget_tokens" => Some(ReasoningMethod::BudgetTokens),
            "adaptive" => Some(ReasoningMethod::Adaptive),
            "unknown" => Some(ReasoningMethod::Unknown),
            _ => None,
        })
        .or_else(|| parse_reasoning_object(metadata_object.get("reasoning")).method)
        .or_else(|| reasoning_options.method.clone())
        .or_else(|| {
            if supported_parameters.iter().any(|p| p == "reasoning_effort") {
                Some(ReasoningMethod::Effort)
            } else if supported_parameters.iter().any(|p| p == "reasoning") {
                Some(ReasoningMethod::Unknown)
            } else {
                None
            }
        });
    // `name` is descriptive only. Keep a record when the upstream catalog
    // omits it so family, dates, and capability metadata still apply.
    let model_name = string("name");

    let direct_or_supported = |key: &str, supported: &str| {
        boolean(key).or_else(|| {
            (!supported_parameters.is_empty()).then(|| {
                supported_parameters
                    .iter()
                    .any(|parameter| parameter == supported)
            })
        })
    };

    let input_modalities = string_array(
        modalities
            .and_then(|modality_object| modality_object.get("input"))
            .or_else(|| {
                architecture
                    .and_then(|architecture_object| architecture_object.get("input_modalities"))
            }),
    );
    let output_modalities = string_array(
        modalities
            .and_then(|modality_object| modality_object.get("output"))
            .or_else(|| {
                architecture
                    .and_then(|architecture_object| architecture_object.get("output_modalities"))
            }),
    );
    let mut reasoning_effort_levels = string_array(
        metadata_object
            .get("reasoning_effort_levels")
            .or_else(|| metadata_object.get("reasoningEffortLevels")),
    );
    if reasoning_effort_levels.is_empty() {
        reasoning_effort_levels = parse_effort_flags(metadata_object);
    }
    if reasoning_effort_levels.is_empty() {
        reasoning_effort_levels = reasoning_options.levels.clone();
    }
    reasoning_effort_levels = normalize_external_levels(reasoning_effort_levels);

    Some(ModelMetadata {
        source_model_id: source_model_id.to_string(),
        provider: provider.to_string(),
        canonical_model_id: string("canonical_model_id").or_else(|| string("canonicalModelId")),
        family: string("family").or_else(|| string("model_family")),
        name: model_name,
        release_date: string("release_date").filter(|date| order::date_key(date).is_some()),
        last_updated: string("last_updated").filter(|date| order::date_key(date).is_some()),
        status: string("status"),
        knowledge: string("knowledge").or_else(|| string("knowledge_cutoff")),
        capabilities: ModelCapabilities {
            reasoning: direct_or_supported("reasoning", "reasoning")
                .or(reasoning_options.supported),
            reasoning_source: string("reasoning_source"),
            reasoning_method,
            reasoning_budget_min_tokens: metadata_object
                .get("reasoning_budget_min_tokens")
                .and_then(Value::as_u64)
                .or(reasoning_options.budget[0]),
            reasoning_budget_max_tokens: metadata_object
                .get("reasoning_budget_max_tokens")
                .and_then(Value::as_u64)
                .or(reasoning_options.budget[1]),
            reasoning_budget_default_tokens: metadata_object
                .get("reasoning_budget_default_tokens")
                .and_then(Value::as_u64)
                .or(reasoning_options.budget[2]),
            reasoning_effort_levels,
            default_reasoning_effort: string("default_reasoning_effort")
                .or_else(|| string("defaultReasoningEffort")),
            tool_call: direct_or_supported("tool_call", "tools"),
            structured_output: direct_or_supported("structured_output", "structured_outputs")
                .or_else(|| {
                    (!supported_parameters.is_empty()).then(|| {
                        supported_parameters
                            .iter()
                            .any(|parameter| parameter == "response_format")
                    })
                }),
            attachment: boolean("attachment"),
            open_weights: boolean("open_weights"),
            input_modalities,
            output_modalities,
            context_limit: limits
                .and_then(|limit_object| limit_object.get("context"))
                .and_then(Value::as_u64)
                .or_else(|| {
                    metadata_object
                        .get("context_length")
                        .and_then(Value::as_u64)
                }),
            input_limit: limits
                .and_then(|limit_object| limit_object.get("input"))
                .and_then(Value::as_u64),
            output_limit: limits
                .and_then(|limit_object| limit_object.get("output"))
                .and_then(Value::as_u64),
        },
    })
}

pub(super) fn string_array(array_value: Option<&Value>) -> Vec<String> {
    array_value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|text| !text.is_empty() && text.len() <= MAX_STRING_LENGTH)
        .map(str::to_string)
        .collect()
}

pub(super) fn validate_metadata_payload(
    metadata_payload: &Value,
) -> Result<Vec<(String, &Value)>, ModelMetadataError> {
    let mut metadata_records = Vec::new();

    if let Some(metadata_object) = metadata_payload.as_object() {
        if let Some(catalog_data) = metadata_object.get("data") {
            let records_array = catalog_data
                .as_array()
                .ok_or(ModelMetadataError::InvalidCatalog)?;
            for record_value in records_array {
                push_catalog_record(&mut metadata_records, record_value)?;
            }
        } else {
            for (id, record_value) in metadata_object {
                if id.trim().is_empty() || id.len() > MAX_STRING_LENGTH {
                    return Err(ModelMetadataError::InvalidCatalog);
                }
                if record_value.as_object().is_none() {
                    return Err(ModelMetadataError::InvalidCatalog);
                }
                metadata_records.push((id.clone(), record_value));
            }
        }
    } else if let Some(records_array) = metadata_payload.as_array() {
        for record_value in records_array {
            push_catalog_record(&mut metadata_records, record_value)?;
        }
    } else {
        return Err(ModelMetadataError::InvalidCatalog);
    }

    if metadata_records.is_empty() || metadata_records.len() > MAX_RECORDS {
        return Err(ModelMetadataError::InvalidCatalog);
    }
    Ok(metadata_records)
}

fn push_catalog_record<'a>(
    metadata_records: &mut Vec<(String, &'a Value)>,
    record_value: &'a Value,
) -> Result<(), ModelMetadataError> {
    let record_object = record_value
        .as_object()
        .ok_or(ModelMetadataError::InvalidCatalog)?;
    let model_id = record_object
        .get("id")
        .and_then(Value::as_str)
        .filter(|model_id| !model_id.trim().is_empty() && model_id.len() <= MAX_STRING_LENGTH)
        .ok_or(ModelMetadataError::InvalidCatalog)?;
    metadata_records.push((model_id.to_string(), record_value));
    Ok(())
}

pub(super) fn metadata_payload_hash(
    metadata_payload: &Value,
) -> Result<String, ModelMetadataError> {
    let serialized_payload =
        serde_json::to_vec(metadata_payload).map_err(|_| ModelMetadataError::InvalidCache)?;
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(serialized_payload))
    ))
}
