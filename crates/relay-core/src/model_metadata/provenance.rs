//! Per-field evidence is built with the normalized merge, not raw export.
use super::MetadataFieldProvenance;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{borrow::Cow, collections::BTreeMap};

pub(super) const MARKER: &str = "_relay_field_provenance";

fn origin_source(origin: &Value) -> &Value {
    if origin.is_string() {
        origin
    } else {
        &origin["source"]
    }
}

fn expand_origin(origin: &mut Value) {
    if origin.is_string() {
        *origin = json!({"source": origin, "revision": null, "conflicts": []});
    }
}

pub(super) fn source(priority: u64) -> &'static str {
    match priority {
        1 => "openrouter",
        2 => "litellm",
        3 => "models_dev_details",
        _ => "models_dev",
    }
}

// Reuse only fixed, known labels. Future/unknown sources stay owned; never
// intern arbitrary external strings or tie a snapshot to its input JSON.
fn retained_source(source: Cow<'_, str>) -> Cow<'static, str> {
    Cow::Borrowed(match source.as_ref() {
        "models_dev" => "models_dev",
        "models_dev_details" => "models_dev_details",
        "openrouter" => "openrouter",
        "litellm" => "litellm",
        "relay_protocol_policy" => "relay_protocol_policy",
        _ => return Cow::Owned(source.into_owned()),
    })
}

pub(super) fn deserialize_source<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Cow<'static, str>, D::Error> {
    Cow::<str>::deserialize(deserializer).map(retained_source)
}

fn is_present_leaf(value: &Value) -> bool {
    !value.is_object()
        && !value.is_null()
        && value.as_str() != Some("")
        && !value.as_array().is_some_and(Vec::is_empty)
}

// The fixed normalized paths contain only object keys, no JSON-pointer escapes
// or array indices. Avoid allocating decoded pointer segments for every lookup.
fn normalized_value<'a>(record: &'a Value, path: &str) -> Option<&'a Value> {
    path.strip_prefix('/')?
        .split('/')
        .try_fold(record, |value, key| value.as_object()?.get(key))
}

pub(super) fn initialize(record: &mut Value, priority: u64) {
    // Only normalized fields can expose provenance. Do not build paths and
    // origin objects for pricing, descriptions or other unused source leaves.
    let mut origins = serde_json::Map::new();
    for (_, paths) in normalized_fields() {
        for &path in paths {
            if !origins.contains_key(path)
                && normalized_value(record, path).is_some_and(is_present_leaf)
            {
                // Expand only when a disagreement needs storing.
                origins.insert(path.into(), Value::String(source(priority).into()));
            }
        }
    }
    record[MARKER] = Value::Object(origins);
}

/// Preserve the winner (including explicit false) and record disagreements.
/// The caller fills only missing values after this function.
pub(super) fn merge(target: &mut Value, fallback: &Value) {
    let Some(origins) = fallback.get(MARKER).and_then(Value::as_object) else {
        return;
    };
    for (field, origin) in origins {
        let Some(incoming) = fallback
            .pointer(field)
            .filter(|value| is_present_leaf(value))
        else {
            continue;
        };
        let existing = target.pointer(field);
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
            target[MARKER][field] = origin.clone();
        } else if existing != Some(incoming) {
            let winner = &mut target[MARKER][field];
            if winner.is_null() {
                *winner = origin.clone();
            }
            expand_origin(winner);
            let conflict = json!({
                "source": origin_source(origin), "revision": null, "value": incoming
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
                "source": origin_source(origin), "revision": origin["revision"], "value": previous
            });
            if !conflicts.contains(&conflict) {
                conflicts.insert(0, conflict);
            }
        }
    }
    conflicts.truncate(4);
    record.entry(MARKER).or_insert_with(|| json!({}))[&path] = if conflicts.is_empty() {
        Value::String(source.to_owned())
    } else {
        json!({"source": source, "revision": null, "conflicts": conflicts})
    };
    record.insert(field.into(), value);
}

pub(super) fn remove_field(record: &mut serde_json::Map<String, Value>, field: &str) {
    record.remove(field);
    if let Some(origins) = record.get_mut(MARKER).and_then(Value::as_object_mut) {
        origins.remove(&format!("/{field}"));
    }
}

/// Keep construction and normalization on the same field/path contract.
fn normalized_fields() -> impl Iterator<Item = (&'static str, &'static [&'static str])> {
    [
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
    ]
    .into_iter()
}

pub(super) fn normalized(record: &Value) -> BTreeMap<Cow<'static, str>, MetadataFieldProvenance> {
    let mut provenance = BTreeMap::new();
    let origins = record.get(MARKER);
    for (field, paths) in normalized_fields() {
        if let Some(origin) = paths.iter().find_map(|path| {
            // Direct public payloads and legacy caches lack merge markers.
            // Read their present fields without cloning/annotating the record.
            let Some(origins) = origins else {
                return normalized_value(record, path)
                    .filter(|value| is_present_leaf(value))
                    .map(|_| MetadataFieldProvenance {
                        source: source(super::reference::identity_priority(record)).into(),
                        revision: None,
                        conflicts: Vec::new(),
                    });
            };
            let raw = origins.get(*path)?;
            if let Some(source) = raw.as_str() {
                Some(MetadataFieldProvenance {
                    source: retained_source(source.into()),
                    revision: None,
                    conflicts: Vec::new(),
                })
            } else {
                MetadataFieldProvenance::deserialize(raw).ok()
            }
        }) {
            provenance.insert(field.into(), origin);
        }
    }
    provenance
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_fields_and_known_sources_reuse_static_labels() {
        for (_, paths) in normalized_fields() {
            let mut record = record_at(paths[0], json!("primary"));
            initialize(&mut record, 0);
            let mut fallback = record_at(paths[0], json!("fallback"));
            initialize(&mut fallback, 2);
            merge(&mut record, &fallback);
            let fields = normalized(&record);
            assert!(!fields.is_empty());
            for (field, origin) in &fields {
                assert!(matches!(field, Cow::Borrowed(_)));
                assert!(matches!(origin.source, Cow::Borrowed("models_dev")));
                assert_eq!(origin.conflicts.len(), 1);
                assert!(matches!(
                    origin.conflicts[0].source,
                    Cow::Borrowed("litellm")
                ));
            }
        }
    }

    #[test]
    fn source_sharing_preserves_json_and_unknown_fields() {
        let value = json!({
            "futureField": {
                "source": "future-source",
                "revision": "future-revision",
                "conflicts": [{"source":"openrouter","revision":null,"value":false}]
            },
            "name": {"source":"models_dev","revision":null}
        });
        let fields: BTreeMap<Cow<'static, str>, MetadataFieldProvenance> =
            serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(fields["futureField"].source, Cow::Owned(_)));
        assert!(matches!(fields["name"].source, Cow::Borrowed("models_dev")));
        assert!(matches!(
            fields["futureField"].conflicts[0].source,
            Cow::Borrowed("openrouter")
        ));
        assert_eq!(serde_json::to_value(&fields).unwrap(), value);

        let mut record = json!({"name":"Synthetic"});
        record[MARKER] = json!({"/name":"future-source"});
        let compact = normalized(&record);
        expand_origin(&mut record[MARKER]["/name"]);
        let expanded = normalized(&record);
        drop(record);
        assert_eq!(compact, expanded);
        assert_eq!(compact["name"].source, "future-source");
    }

    // The previous all-leaf representation is kept only as a compatibility
    // oracle. Production must not walk/annotate unused source metadata.
    fn initialize_all_leaves(record: &mut Value, priority: u64) {
        fn collect(
            value: &Value,
            path: String,
            origins: &mut serde_json::Map<String, Value>,
            priority: u64,
        ) {
            if let Some(object) = value.as_object() {
                for (key, value) in object {
                    if !key.starts_with("_relay_") {
                        collect(value, format!("{path}/{key}"), origins, priority);
                    }
                }
            } else if is_present_leaf(value) {
                origins.insert(path, source(priority).into());
            }
        }
        let mut origins = serde_json::Map::new();
        collect(record, String::new(), &mut origins, priority);
        record[MARKER] = origins.into();
    }

    fn record_at(path: &str, value: Value) -> Value {
        path.trim_start_matches('/')
            .rsplit('/')
            .fold(value, |value, key| {
                Value::Object([(key.to_owned(), value)].into_iter().collect())
            })
    }

    #[test]
    fn tracked_fields_match_all_leaf_and_direct_payload_normalization() {
        for (_, paths) in normalized_fields() {
            for path in paths {
                for value in [
                    Value::Null,
                    json!(""),
                    json!({}),
                    json!([]),
                    json!(false),
                    json!(0),
                    json!("value"),
                    json!(["value"]),
                ] {
                    let direct = record_at(path, value);
                    assert!(
                        !path.contains('~'),
                        "normalized paths cannot contain escapes"
                    );
                    assert_eq!(normalized_value(&direct, path), direct.pointer(path));
                    let mut legacy = direct.clone();
                    initialize_all_leaves(&mut legacy, 0);
                    let mut tracked = direct.clone();
                    initialize(&mut tracked, 0);
                    assert_eq!(normalized(&tracked), normalized(&legacy), "{path}");
                    assert_eq!(normalized(&direct), normalized(&legacy), "{path}");
                }
            }
        }
    }

    #[test]
    fn tracked_merges_preserve_false_empty_lists_priorities_and_conflicts() {
        for (_, paths) in normalized_fields() {
            for path in paths {
                for (primary, fallback) in [
                    (json!(false), json!(true)),
                    (json!([]), json!(["fast"])),
                    (Value::Null, json!(0)),
                    (json!("winner"), json!("other")),
                ] {
                    let mut old_target = record_at(path, primary);
                    let mut old_fallback = record_at(path, fallback);
                    let mut target = old_target.clone();
                    let mut fallback = old_fallback.clone();
                    initialize_all_leaves(&mut old_target, 0);
                    initialize_all_leaves(&mut old_fallback, 2);
                    initialize(&mut target, 0);
                    initialize(&mut fallback, 2);
                    merge(&mut old_target, &old_fallback);
                    merge(&mut target, &fallback);
                    assert_eq!(normalized(&target), normalized(&old_target), "{path}");
                    // Repeating an identical source must not duplicate conflicts.
                    let once = normalized(&target);
                    merge(&mut target, &fallback);
                    assert_eq!(normalized(&target), once, "{path}");
                }
            }
        }
    }

    #[test]
    fn unused_source_metadata_does_not_allocate_origins() {
        let mut record = json!({"name":"Synthetic","reasoning":false,"limit":{"context":1024},"description":"x".repeat(100_000)});
        record["unused"] = Value::Object(
            (0..1024)
                .map(|i| (format!("field-{i}"), json!({"detail": i})))
                .collect(),
        );
        let mut legacy = record.clone();
        initialize_all_leaves(&mut legacy, 0);
        initialize(&mut record, 0);
        assert_eq!(record[MARKER].as_object().unwrap().len(), 3);
        assert_eq!(legacy[MARKER].as_object().unwrap().len(), 1028);
        assert_eq!(normalized(&record), normalized(&legacy));
        let mut legacy_fallback = record.clone();
        legacy_fallback.as_object_mut().unwrap().remove(MARKER);
        assert_eq!(normalized(&legacy_fallback), normalized(&record));
    }

    #[test]
    fn compact_origins_preserve_conflicts_and_legacy_normalization() {
        let mut target = json!({"reasoning":false,"tool_call":true});
        initialize(&mut target, 0);
        assert!(target[MARKER]
            .as_object()
            .unwrap()
            .values()
            .all(Value::is_string));
        let mut fallback = json!({"reasoning":true,"attachment":true});
        initialize(&mut fallback, 2);
        merge(&mut target, &fallback);
        target["attachment"] = json!(true);
        let compact = normalized(&target);
        let reasoning = &compact["capabilities.reasoning"];
        assert_eq!(reasoning.source, "models_dev");
        assert_eq!(reasoning.conflicts.len(), 1);
        assert_eq!(reasoning.conflicts[0].source, "litellm");
        assert_eq!(reasoning.conflicts[0].value, json!(true));
        let mut legacy = target.clone();
        for origin in legacy[MARKER].as_object_mut().unwrap().values_mut() {
            expand_origin(origin);
        }
        assert_eq!(compact, normalized(&legacy));
        set_field(
            target.as_object_mut().unwrap(),
            "tool_call",
            json!(false),
            "openrouter",
        );
        let changed = normalized(&target);
        assert_eq!(
            changed["capabilities.toolCall"].conflicts[0].source,
            "models_dev"
        );
    }
}
