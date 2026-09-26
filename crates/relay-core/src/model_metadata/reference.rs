use super::{normalize, validate_payload, MAX_RECORDS};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const IDENTITY_PRIORITY: &str = "_relay_identity_priority";

pub(super) fn identity_priority(value: &Value) -> u64 {
    value
        .get(IDENTITY_PRIORITY)
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// Fixed reference sources fill missing fields, never participant /models.
/// Keep primary fields (including false) and supplement partial model records.
pub(super) fn merge_reference_records(
    primary: &Value,
    details: Option<&Value>,
    openrouter: Option<&Value>,
    litellm: Option<&Value>,
) -> BTreeMap<String, Value> {
    let mut records = BTreeMap::new();
    for (id, value) in validate_payload(primary).unwrap_or_default() {
        insert(&mut records, &id, value.clone(), 0);
    }
    for (provider, data) in details.and_then(Value::as_object).into_iter().flatten() {
        for (id, record) in data
            .get("models")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let id = record
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .unwrap_or(id);
            let qualified = if id.contains('/') {
                id.to_owned()
            } else {
                format!("{provider}/{id}")
            };
            insert(&mut records, &qualified, record.clone(), 3);
        }
    }
    for record in openrouter
        .and_then(|v| v.get("data"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(id) = record.get("id").and_then(Value::as_str) {
            let mut record = record.clone();
            // Normalize the reference's field names before supplementing a
            // primary row. Otherwise an empty modalities/limit field could
            // shadow the useful fallback under architecture/context_length.
            let mut limits = Map::new();
            if let Some(context) = record
                .get("context_length")
                .and_then(Value::as_u64)
                .filter(|v| *v > 0)
            {
                limits.insert("context".into(), json!(context));
            }
            if let Some(output) = record
                .pointer("/top_provider/max_completion_tokens")
                .and_then(Value::as_u64)
                .filter(|v| *v > 0)
            {
                limits.insert("output".into(), json!(output));
            }
            if !limits.is_empty() {
                record["limit"] = Value::Object(limits);
            }
            for (from, to) in [
                ("input_modalities", "input"),
                ("output_modalities", "output"),
            ] {
                if let Some(modalities) = record
                    .get("architecture")
                    .and_then(|a| a.get(from))
                    .cloned()
                {
                    if !record.get("modalities").is_some_and(Value::is_object) {
                        record["modalities"] = json!({});
                    }
                    record["modalities"][to] = modalities;
                }
            }
            insert(&mut records, id, record, 1);
        }
    }
    for (id, record) in litellm.and_then(Value::as_object).into_iter().flatten() {
        if id == "sample_spec" {
            continue;
        }
        let Some(provider) = record.get("litellm_provider").and_then(Value::as_str) else {
            continue;
        };
        let id = if id.contains('/') {
            id.clone()
        } else {
            format!("{provider}/{id}")
        };
        let mut projected = Map::new();
        for (from, to) in [
            ("supports_function_calling", "tool_call"),
            ("supports_response_schema", "structured_output"),
            ("supports_vision", "attachment"),
            ("supports_reasoning", "reasoning"),
        ] {
            if let Some(value) = record.get(from).and_then(Value::as_bool) {
                projected.insert(to.into(), json!(value));
            }
        }
        let mut limits = Map::new();
        for (from, to) in [
            ("max_tokens", "context"),
            ("max_input_tokens", "input"),
            ("max_output_tokens", "output"),
        ] {
            if let Some(value) = record.get(from).and_then(Value::as_u64).filter(|v| *v > 0) {
                limits.insert(to.into(), json!(value));
            }
        }
        if !limits.is_empty() {
            projected.insert("limit".into(), Value::Object(limits));
        }
        insert(&mut records, &id, Value::Object(projected), 2);
    }
    records
}

fn insert(records: &mut BTreeMap<String, Value>, id: &str, mut value: Value, priority: u64) {
    if !crate::is_valid_model_token(id) || !value.is_object() {
        return;
    }
    let id = normalize(id);
    value[IDENTITY_PRIORITY] = json!(priority);
    if value.get("reasoning").is_some() && value.get("reasoning_source").is_none() {
        value["reasoning_source"] = json!(match priority {
            1 => "openrouter",
            2 => "litellm",
            _ => "models_dev",
        });
    }
    if let Some(record) = records.get_mut(&id) {
        record[IDENTITY_PRIORITY] = json!(identity_priority(record).min(priority));
        fill_missing(record, value);
    } else if records.len() < MAX_RECORDS {
        records.insert(id, value);
    }
}

fn fill_missing(target: &mut Value, fallback: Value) {
    if target.is_null()
        || target.as_str() == Some("")
        || target.as_array().is_some_and(Vec::is_empty)
    {
        *target = fallback;
        return;
    }
    if let (Some(target), Value::Object(fallback)) = (target.as_object_mut(), fallback) {
        for (key, value) in fallback {
            if let Some(existing) = target.get_mut(&key) {
                fill_missing(existing, value);
            } else {
                target.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_sources_fill_all_metadata_and_keep_explicit_primary_values() {
        let records = merge_reference_records(
            &json!({"openai/gpt-synthetic":{"tool_call":false,"limit":{"context":100}}}),
            Some(
                &json!({"openai":{"models":{"gpt-synthetic":{"family":"future","name":"Synthetic","limit":{"output":20}}}}}),
            ),
            Some(
                &json!({"data":[{"id":"openai/gpt-synthetic","architecture":{"input_modalities":["text","image"]}}, {"id":"other/new-model","name":"New"}]}),
            ),
            Some(
                &json!({"gpt-synthetic":{"litellm_provider":"openai","supports_function_calling":true,"supports_response_schema":true,"max_tokens":999,"max_input_tokens":80}}),
            ),
        );
        let catalog = super::super::ModelMetadataCatalog::from_models_dev_json(
            &serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        let model = catalog.resolve("gpt-synthetic").unwrap();
        assert_eq!(model.family.as_deref(), Some("future"));
        let capabilities = catalog.capabilities_for("gpt-synthetic");
        assert_eq!(capabilities.tool_call, Some(false));
        assert_eq!(capabilities.structured_output, Some(true));
        assert_eq!(capabilities.input_modalities, ["text", "image"]);
        assert_eq!(capabilities.context_limit, Some(100));
        assert_eq!(capabilities.input_limit, Some(80));
        assert_eq!(capabilities.output_limit, Some(20));
        assert_eq!(
            catalog.resolve("new-model").unwrap().name.as_deref(),
            Some("New")
        );
        assert!(catalog.resolve("not-in-references").is_none());
    }

    #[test]
    fn supplemental_hosts_do_not_hide_primary_identity_or_collapse_ambiguous_models() {
        let records = merge_reference_records(
            &json!({"openai/future":{"name":"Canonical"},"alpha/shared":{},"beta/shared":{}}),
            Some(&json!({
                "reseller":{"models":{"future":{"name":"Hosted"}}},
                "router":{"models":{"alias":{"id":"openai/future","family":"future-family"}}}
            })),
            Some(&json!({"data":[{"id":"other/shared"},{"id":"vendor/new-model","name":"New"}]})),
            None,
        );
        let catalog = super::super::ModelMetadataCatalog::from_models_dev_json(
            &serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        assert_eq!(catalog.resolve("future").unwrap().provider, "openai");
        assert_eq!(
            catalog.resolve("future").unwrap().family.as_deref(),
            Some("future-family")
        );
        assert_eq!(
            catalog.resolve("reseller/future").unwrap().name.as_deref(),
            Some("Hosted")
        );
        assert!(catalog.resolve("shared").is_none());
        assert!(catalog.resolve("alpha/shared").is_some());
        assert_eq!(
            catalog.resolve("new-model").unwrap().name.as_deref(),
            Some("New")
        );
        assert!(!records.contains_key("router/openai/future"));
    }

    #[test]
    fn missing_primary_uses_qualified_reference_identity_before_hosting_aliases() {
        let records = merge_reference_records(
            &json!({}),
            Some(&json!({"host":{"models":{"future":{"name":"Hosted"}}}})),
            Some(&json!({"data":[{"id":"vendor/future","name":"Reference"}]})),
            None,
        );
        let catalog = super::super::ModelMetadataCatalog::from_models_dev_json(
            &serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        assert_eq!(catalog.resolve("future").unwrap().provider, "vendor");
        assert_eq!(
            catalog.resolve("host/future").unwrap().name.as_deref(),
            Some("Hosted")
        );
    }

    #[test]
    fn malformed_optional_reference_fields_do_not_break_valid_fallback_data() {
        let records = merge_reference_records(
            &json!({"vendor/model":{"modalities":{"input":[]}}}),
            None,
            Some(&json!({"data":[{"id":"vendor/model","modalities":"invalid",
                "architecture":{"input_modalities":["text"]},"context_length":256}]})),
            None,
        );
        let catalog = super::super::ModelMetadataCatalog::from_models_dev_json(
            &serde_json::to_string(&records).unwrap(),
        )
        .unwrap();
        let model = catalog.capabilities_for("model");
        assert_eq!(model.input_modalities, ["text"]);
        assert_eq!(model.context_limit, Some(256));
    }
}
