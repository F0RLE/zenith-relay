use serde_json::{json, Map, Value};

pub(super) fn required_string(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry.get(key).is_some_and(Value::is_string)
}

pub(super) fn required_non_empty_string(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|text_value| {
            !text_value.trim().is_empty() && !text_value.chars().any(char::is_control)
        })
}

pub(super) fn optional_string(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry
        .get(key)
        .is_none_or(|field_value| field_value.is_null() || field_value.is_string())
}

pub(in crate::catalog::codex) fn optional_non_empty_string(
    catalog_entry: &Map<String, Value>,
    key: &str,
) -> bool {
    catalog_entry.get(key).is_none_or(|field_value| {
        field_value.is_null()
            || field_value.as_str().is_some_and(|text_value| {
                !text_value.is_empty() && !text_value.chars().any(char::is_control)
            })
    })
}

pub(super) fn required_bool(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry.get(key).is_some_and(Value::is_boolean)
}

pub(super) fn default_bool(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry.get(key).is_none_or(Value::is_boolean)
}

pub(super) fn required_i32(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry
        .get(key)
        .and_then(Value::as_i64)
        .is_some_and(|numeric_value| i32::try_from(numeric_value).is_ok())
}

pub(super) fn optional_i64(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry
        .get(key)
        .is_none_or(|field_value| field_value.is_null() || field_value.as_i64().is_some())
}

pub(in crate::catalog::codex) fn default_string_array(
    catalog_entry: &Map<String, Value>,
    key: &str,
) -> bool {
    catalog_entry.get(key).is_none_or(|array_value| {
        array_value
            .as_array()
            .is_some_and(|array_items| array_items.iter().all(Value::is_string))
    })
}

pub(super) fn default_enum_string(
    catalog_entry: &Map<String, Value>,
    key: &str,
    accepted: &[&str],
) -> bool {
    catalog_entry.get(key).is_none_or(|field_value| {
        field_value
            .as_str()
            .is_some_and(|enum_text| accepted.contains(&enum_text))
    })
}

pub(super) fn enum_string(
    catalog_entry: &Map<String, Value>,
    key: &str,
    accepted: &[&str],
    required: bool,
) -> bool {
    match catalog_entry.get(key) {
        Some(Value::Null) if !required => true,
        Some(field_value) => field_value
            .as_str()
            .is_some_and(|enum_text| accepted.contains(&enum_text)),
        None => !required,
    }
}

pub(super) fn valid_reasoning_level(reasoning_level_value: &Value) -> bool {
    reasoning_level_value.as_object().is_some_and(|level| {
        level
            .get("effort")
            .and_then(Value::as_str)
            .is_some_and(|effort| {
                !effort.is_empty() && effort.len() <= 64 && !effort.chars().any(char::is_control)
            })
            && required_string(level, "description")
    })
}

pub(super) fn default_service_tiers(catalog_entry: &Map<String, Value>) -> bool {
    catalog_entry
        .get("service_tiers")
        .is_none_or(|service_tiers_value| {
            service_tiers_value.as_array().is_some_and(|tiers| {
                tiers.iter().all(|tier| {
                    tier.as_object().is_some_and(|tier| {
                        required_string(tier, "id")
                            && required_string(tier, "name")
                            && required_string(tier, "description")
                    })
                })
            })
        })
}

pub(super) fn optional_message_object(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry.get(key).is_none_or(|field_value| {
        field_value.is_null()
            || field_value
                .as_object()
                .is_some_and(|message_object| required_string(message_object, "message"))
    })
}

pub(super) fn optional_upgrade(catalog_entry: &Map<String, Value>) -> bool {
    catalog_entry.get("upgrade").is_none_or(|upgrade_value| {
        upgrade_value.is_null()
            || upgrade_value.as_object().is_some_and(|upgrade| {
                required_string(upgrade, "model") && required_string(upgrade, "migration_markdown")
            })
    })
}

pub(super) fn optional_model_messages(catalog_entry: &Map<String, Value>) -> bool {
    catalog_entry
        .get("model_messages")
        .is_none_or(|messages_value| {
            messages_value.is_null()
                || messages_value.as_object().is_some_and(|messages| {
                    optional_string(messages, "instructions_template")
                        && optional_string_map(messages, "instructions_variables")
                        && optional_string_map(messages, "approvals")
                        && optional_string_map(messages, "collaboration_modes")
                        && optional_string_map(messages, "auto_review")
                        && optional_string_map(messages, "permissions")
                        && optional_token_budget(messages)
                })
        })
}

pub(super) fn optional_string_map(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry.get(key).is_none_or(|map_value| {
        map_value.is_null()
            || map_value.as_object().is_some_and(|string_map| {
                string_map
                    .values()
                    .all(|map_value| map_value.is_null() || map_value.is_string())
            })
    })
}

pub(super) fn optional_token_budget(catalog_entry: &Map<String, Value>) -> bool {
    catalog_entry
        .get("token_budget")
        .is_none_or(|budget_value| {
            budget_value.is_null()
                || budget_value.as_object().is_some_and(|budget| {
                    required_i64(budget, "reminder_threshold_tokens")
                        && required_string(budget, "reminder_message_template")
                        && required_string(budget, "guidance_message")
                        && required_string(budget, "auto_compact_fallback_prompt")
                        && required_i64(budget, "auto_compact_fallback_buffer_tokens")
                })
        })
}

pub(super) fn required_i64(catalog_entry: &Map<String, Value>, key: &str) -> bool {
    catalog_entry.get(key).and_then(Value::as_i64).is_some()
}

pub(in crate::catalog::codex) fn valid_truncation_policy(
    catalog_entry: &Map<String, Value>,
) -> bool {
    // The absence of this field is intentional for routed/API models: Relay
    // must not impose a synthetic request-size limit on an upstream that may
    // support a larger context. If a provider-owned catalog supplies the
    // policy, still validate it strictly instead of accepting malformed data.
    catalog_entry
        .get("truncation_policy")
        .is_some_and(|policy_value| {
            policy_value.as_object().is_some_and(|policy| {
                enum_string(policy, "mode", &["bytes", "tokens"], true)
                    && required_i64(policy, "limit")
            })
        })
}

const UPSTREAM_REASONING_LEVEL_KEYS: &[&str] = &[
    "supported_reasoning_levels",
    "supportedReasoningLevels",
    "supported_reasoning_efforts",
    "supportedReasoningEfforts",
    "efforts",
    "reasoning_efforts",
    "reasoningEfforts",
    "reasoning_effort_options",
    "reasoningEffortOptions",
    "reasoning_effort_modes",
    "reasoningEffortModes",
];

/// Reports whether a source row explicitly attempted to describe reasoning.
/// An empty or malformed declaration is still deliberate and remains empty.
pub fn source_row_declares_reasoning(template: &Map<String, Value>) -> bool {
    UPSTREAM_REASONING_LEVEL_KEYS
        .iter()
        .copied()
        .chain(["supports_reasoning_effort", "supportsReasoningEffort"])
        .any(|key| template.contains_key(key))
}

pub(in crate::catalog::codex) fn upstream_reasoning_levels(
    template: &Map<String, Value>,
) -> Option<Value> {
    if ["supports_reasoning_effort", "supportsReasoningEffort"]
        .into_iter()
        .find_map(|key| template.get(key).and_then(Value::as_bool))
        == Some(false)
    {
        return Some(Value::Array(Vec::new()));
    }
    let raw_levels = UPSTREAM_REASONING_LEVEL_KEYS
        .iter()
        .find_map(|key| template.get(*key))?;
    let levels = raw_levels.as_array()?;
    let normalized = levels
        .iter()
        .filter_map(|level| {
            let (effort, description) = match level {
                Value::String(effort) => (effort.as_str(), effort.as_str()),
                Value::Object(level) => {
                    let effort = level
                        .get("effort")
                        .or_else(|| level.get("id"))
                        .or_else(|| level.get("value"))
                        .and_then(Value::as_str)?;
                    let description = level
                        .get("description")
                        .or_else(|| level.get("label"))
                        .and_then(Value::as_str)
                        .unwrap_or(effort);
                    (effort, description)
                }
                _ => return None,
            };
            let effort = effort.trim();
            let description = description.trim();
            (valid_reasoning_effort_text(effort) && !description.is_empty())
                .then(|| json!({"effort": effort, "description": description}))
        })
        .collect::<Vec<_>>();
    (normalized.len() == levels.len()).then_some(Value::Array(normalized))
}

pub(super) fn valid_reasoning_effort_text(effort_text: &str) -> bool {
    !effort_text.is_empty() && effort_text.len() <= 64 && !effort_text.chars().any(char::is_control)
}

pub(super) fn valid_input_modalities(catalog_entry: &Map<String, Value>) -> bool {
    catalog_entry
        .get("input_modalities")
        .is_none_or(|modalities_value| {
            modalities_value.as_array().is_some_and(|modalities| {
                modalities.iter().all(|modality| {
                    modality.as_str().is_some_and(|modality_name| {
                        matches!(modality_name, "text" | "image" | "audio")
                    })
                })
            })
        })
}
