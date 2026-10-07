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
    value: &Value,
) -> Option<ModelMetadata> {
    let object = value.as_object()?;
    let string = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty() && value.len() <= MAX_STRING_LENGTH)
            .map(str::to_string)
    };
    let boolean = |key: &str| object.get(key).and_then(Value::as_bool);
    let modalities = object.get("modalities").and_then(Value::as_object);
    let architecture = object.get("architecture").and_then(Value::as_object);
    let limits = object.get("limit").and_then(Value::as_object);
    let supported_parameters = string_array(object.get("supported_parameters"));
    let reasoning_options = parse_reasoning_options(
        object
            .get("reasoning_options")
            .or_else(|| object.get("reasoningOptions")),
    );
    let reasoning_method = object
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
        .or_else(|| parse_reasoning_object(object.get("reasoning")).method)
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
    let name = string("name");

    let direct_or_supported = |key: &str, supported: &str| {
        boolean(key).or_else(|| {
            (!supported_parameters.is_empty())
                .then(|| supported_parameters.iter().any(|value| value == supported))
        })
    };

    let input_modalities = string_array(
        modalities
            .and_then(|value| value.get("input"))
            .or_else(|| architecture.and_then(|value| value.get("input_modalities"))),
    );
    let output_modalities = string_array(
        modalities
            .and_then(|value| value.get("output"))
            .or_else(|| architecture.and_then(|value| value.get("output_modalities"))),
    );
    let mut reasoning_effort_levels = string_array(
        object
            .get("reasoning_effort_levels")
            .or_else(|| object.get("reasoningEffortLevels")),
    );
    if reasoning_effort_levels.is_empty() {
        reasoning_effort_levels = parse_effort_flags(object);
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
        name,
        release_date: string("release_date").filter(|date| order::date_key(date).is_some()),
        last_updated: string("last_updated").filter(|date| order::date_key(date).is_some()),
        status: string("status"),
        knowledge: string("knowledge").or_else(|| string("knowledge_cutoff")),
        capabilities: ModelCapabilities {
            reasoning: direct_or_supported("reasoning", "reasoning")
                .or(reasoning_options.supported),
            reasoning_source: string("reasoning_source"),
            reasoning_method,
            reasoning_budget_min_tokens: object
                .get("reasoning_budget_min_tokens")
                .and_then(Value::as_u64)
                .or(reasoning_options.budget[0]),
            reasoning_budget_max_tokens: object
                .get("reasoning_budget_max_tokens")
                .and_then(Value::as_u64)
                .or(reasoning_options.budget[1]),
            reasoning_budget_default_tokens: object
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
                            .any(|value| value == "response_format")
                    })
                }),
            attachment: boolean("attachment"),
            open_weights: boolean("open_weights"),
            input_modalities,
            output_modalities,
            context_limit: limits
                .and_then(|value| value.get("context"))
                .and_then(Value::as_u64)
                .or_else(|| object.get("context_length").and_then(Value::as_u64)),
            input_limit: limits
                .and_then(|value| value.get("input"))
                .and_then(Value::as_u64),
            output_limit: limits
                .and_then(|value| value.get("output"))
                .and_then(Value::as_u64),
        },
    })
}

pub(super) fn string_array(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter(|value| !value.is_empty() && value.len() <= MAX_STRING_LENGTH)
        .map(str::to_string)
        .collect()
}

pub(super) fn validate_payload(
    payload: &Value,
) -> Result<Vec<(String, &Value)>, ModelMetadataError> {
    let mut records = Vec::new();

    if let Some(object) = payload.as_object() {
        if let Some(data) = object.get("data") {
            let array = data.as_array().ok_or(ModelMetadataError::InvalidCatalog)?;
            for value in array {
                push_catalog_record(&mut records, value)?;
            }
        } else {
            for (id, value) in object {
                if id.trim().is_empty() || id.len() > MAX_STRING_LENGTH {
                    return Err(ModelMetadataError::InvalidCatalog);
                }
                if value.as_object().is_none() {
                    return Err(ModelMetadataError::InvalidCatalog);
                }
                records.push((id.clone(), value));
            }
        }
    } else if let Some(array) = payload.as_array() {
        for value in array {
            push_catalog_record(&mut records, value)?;
        }
    } else {
        return Err(ModelMetadataError::InvalidCatalog);
    }

    if records.is_empty() || records.len() > MAX_RECORDS {
        return Err(ModelMetadataError::InvalidCatalog);
    }
    Ok(records)
}

fn push_catalog_record<'a>(
    records: &mut Vec<(String, &'a Value)>,
    value: &'a Value,
) -> Result<(), ModelMetadataError> {
    let record = value
        .as_object()
        .ok_or(ModelMetadataError::InvalidCatalog)?;
    let id = record
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && id.len() <= MAX_STRING_LENGTH)
        .ok_or(ModelMetadataError::InvalidCatalog)?;
    records.push((id.to_string(), value));
    Ok(())
}

pub(super) fn payload_hash(payload: &Value) -> Result<String, ModelMetadataError> {
    let bytes = serde_json::to_vec(payload).map_err(|_| ModelMetadataError::InvalidCache)?;
    Ok(format!("sha256:{}", hex::encode(Sha256::digest(bytes))))
}
