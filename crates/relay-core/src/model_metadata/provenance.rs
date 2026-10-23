//! Per-field evidence is built with the normalized merge, not raw export.
use super::MetadataFieldProvenance;
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub(super) const MARKER: &str = "_relay_field_provenance";

pub(super) fn source(priority: u64) -> &'static str {
    match priority {
        1 => "openrouter",
        2 => "litellm",
        3 => "models_dev_details",
        _ => "models_dev",
    }
}

fn leaf_values<'a>(value: &'a Value, path: String, fields: &mut BTreeMap<String, &'a Value>) {
    if let Some(object) = value.as_object() {
        for (key, value) in object {
            if !key.starts_with("_relay_") {
                leaf_values(value, format!("{path}/{key}"), fields);
            }
        }
    } else if !value.is_null()
        && value.as_str() != Some("")
        && !value.as_array().is_some_and(Vec::is_empty)
    {
        fields.insert(path, value);
    }
}

pub(super) fn initialize(record: &mut Value, priority: u64) {
    let mut leaves = BTreeMap::new();
    leaf_values(record, String::new(), &mut leaves);
    let origins = leaves
        .keys()
        .map(|field| {
            (
                field.clone(),
                json!({"source": source(priority), "revision": null, "conflicts": []}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    record[MARKER] = Value::Object(origins);
}

/// Preserve the winner (including explicit false) and record disagreements.
/// The caller fills only missing values after this function.
pub(super) fn merge(target: &mut Value, fallback: &Value) {
    let mut leaves = BTreeMap::new();
    leaf_values(fallback, String::new(), &mut leaves);
    for (field, incoming) in leaves {
        let Some(origin) = fallback
            .get(MARKER)
            .and_then(|origins| origins.get(&field))
            .cloned()
        else {
            continue;
        };
        let existing = target.pointer(&field);
        let missing = existing.is_none_or(|value| {
            value.is_null()
                || value.as_str() == Some("")
                || (value.as_array().is_some_and(Vec::is_empty)
                    && !matches!(
                        field.as_str(),
                        "/service_tiers" | "/supported_service_tiers"
                    ))
        });
        if missing {
            target[MARKER][&field] = origin;
        } else if existing != Some(incoming) {
            let winner = &mut target[MARKER][&field];
            if winner.is_null() {
                *winner = origin.clone();
            }
            let conflict = json!({
                "source": origin["source"], "revision": null, "value": incoming
            });
            if let Some(conflicts) = winner.get_mut("conflicts").and_then(Value::as_array_mut) {
                if !conflicts.contains(&conflict) && conflicts.len() < 4 {
                    conflicts.push(conflict);
                }
            }
        }
    }
}

/// Reasoning enrichment has its own evidence order. Record that decision at
/// the field being changed, preserving the previous winner as a disagreement.
pub(super) fn set_field(
    record: &mut serde_json::Map<String, Value>,
    field: &str,
    value: Value,
    source: &str,
) {
    let path = format!("/{field}");
    let previous = record.get(field).cloned();
    let old_origin = record
        .get(MARKER)
        .and_then(|origins| origins.get(&path))
        .cloned();
    let mut conflicts = old_origin
        .as_ref()
        .and_then(|origin| origin.get("conflicts"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    conflicts.retain(|conflict| conflict["source"] != source || conflict["value"] != value);
    if let (Some(previous), Some(origin)) = (previous, old_origin.as_ref()) {
        if previous != value {
            let conflict = json!({
                "source": origin["source"], "revision": origin["revision"], "value": previous
            });
            if !conflicts.contains(&conflict) {
                conflicts.insert(0, conflict);
            }
        }
    }
    conflicts.truncate(4);
    record.entry(MARKER).or_insert_with(|| json!({}))[&path] =
        json!({"source": source, "revision": null, "conflicts": conflicts});
    record.insert(field.into(), value);
}

pub(super) fn remove_field(record: &mut serde_json::Map<String, Value>, field: &str) {
    record.remove(field);
    if let Some(origins) = record.get_mut(MARKER).and_then(Value::as_object_mut) {
        origins.remove(&format!("/{field}"));
    }
}

pub(super) fn normalized(record: &Value) -> BTreeMap<String, MetadataFieldProvenance> {
    let mut provenance = BTreeMap::new();
    for (field, paths) in [
        ("name", &["/name"][..]),
        ("family", &["/family", "/model_family"]),
        (
            "canonicalModelId",
            &["/canonical_model_id", "/canonicalModelId"],
        ),
        ("aliases", &["/aliases"]),
        (
            "protocolPreferences",
            &["/protocol_preferences", "/protocolPreferences"],
        ),
        ("releaseDate", &["/release_date"]),
        ("lastUpdated", &["/last_updated"]),
        ("status", &["/status"]),
        ("knowledge", &["/knowledge", "/knowledge_cutoff"]),
        (
            "capabilities.reasoning",
            &["/reasoning", "/supported_parameters"],
        ),
        (
            "capabilities.reasoningMethod",
            &[
                "/reasoning_method",
                "/reasoning_options",
                "/supported_parameters",
            ],
        ),
        (
            "capabilities.reasoningEffortLevels",
            &["/reasoning_effort_levels", "/reasoningEffortLevels"],
        ),
        (
            "capabilities.defaultReasoningEffort",
            &["/default_reasoning_effort", "/defaultReasoningEffort"],
        ),
        (
            "capabilities.reasoningBudgetMinTokens",
            &["/reasoning_budget_min_tokens"],
        ),
        (
            "capabilities.reasoningBudgetMaxTokens",
            &["/reasoning_budget_max_tokens"],
        ),
        (
            "capabilities.reasoningBudgetDefaultTokens",
            &["/reasoning_budget_default_tokens"],
        ),
        (
            "capabilities.toolCall",
            &["/tool_call", "/supported_parameters"],
        ),
        (
            "capabilities.structuredOutput",
            &["/structured_output", "/supported_parameters"],
        ),
        ("capabilities.attachment", &["/attachment"]),
        ("capabilities.openWeights", &["/open_weights"]),
        (
            "capabilities.temperature",
            &["/temperature", "/supported_parameters"],
        ),
        ("capabilities.promptCaching", &["/prompt_caching"]),
        (
            "capabilities.parallelToolCall",
            &["/parallel_tool_call", "/supported_parameters"],
        ),
        (
            "capabilities.toolChoice",
            &["/tool_choice", "/supported_parameters"],
        ),
        ("capabilities.streaming", &["/streaming"]),
        (
            "capabilities.serviceTiers",
            &["/service_tiers", "/supported_service_tiers"],
        ),
        (
            "capabilities.inputModalities",
            &["/modalities/input", "/architecture/input_modalities"],
        ),
        (
            "capabilities.outputModalities",
            &["/modalities/output", "/architecture/output_modalities"],
        ),
        (
            "capabilities.contextLimit",
            &["/limit/context", "/context_length"],
        ),
        ("capabilities.inputLimit", &["/limit/input"]),
        (
            "capabilities.outputLimit",
            &["/limit/output", "/top_provider/max_completion_tokens"],
        ),
    ] {
        if let Some(origin) = paths.iter().find_map(|path| {
            let raw = record.get(MARKER)?.get(*path)?;
            serde_json::from_value::<MetadataFieldProvenance>(raw.clone()).ok()
        }) {
            provenance.insert(field.into(), origin);
        }
    }
    // Legacy caches and direct public payloads predate merge markers.
    // Attribute only fields present in the source, never inferred defaults.
    if record.get(MARKER).is_none() {
        let mut annotated = record.clone();
        initialize(&mut annotated, super::reference::identity_priority(record));
        provenance = normalized(&annotated);
    }
    provenance
}
