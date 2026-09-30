use serde_json::{json, Map, Value};

pub(super) fn required_string(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_some_and(Value::is_string)
}

pub(super) fn required_non_empty_string(entry: &Map<String, Value>, key: &str) -> bool {
    entry
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
}

pub(super) fn optional_string(entry: &Map<String, Value>, key: &str) -> bool {
    entry
        .get(key)
        .is_none_or(|value| value.is_null() || value.is_string())
}

pub(in crate::catalog::codex) fn optional_non_empty_string(
    entry: &Map<String, Value>,
    key: &str,
) -> bool {
    entry.get(key).is_none_or(|value| {
        value.is_null()
            || value
                .as_str()
                .is_some_and(|value| !value.is_empty() && !value.chars().any(char::is_control))
    })
}

pub(super) fn required_bool(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_some_and(Value::is_boolean)
}

pub(super) fn default_bool(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(Value::is_boolean)
}

pub(super) fn required_i32(entry: &Map<String, Value>, key: &str) -> bool {
    entry
        .get(key)
        .and_then(Value::as_i64)
        .is_some_and(|value| i32::try_from(value).is_ok())
}

pub(super) fn optional_i64(entry: &Map<String, Value>, key: &str) -> bool {
    entry
        .get(key)
        .is_none_or(|value| value.is_null() || value.as_i64().is_some())
}

pub(in crate::catalog::codex) fn default_string_array(
    entry: &Map<String, Value>,
    key: &str,
) -> bool {
    entry.get(key).is_none_or(|value| {
        value
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string))
    })
}

pub(super) fn default_enum_string(
    entry: &Map<String, Value>,
    key: &str,
    accepted: &[&str],
) -> bool {
    entry.get(key).is_none_or(|value| {
        value
            .as_str()
            .is_some_and(|value| accepted.contains(&value))
    })
}

pub(super) fn enum_string(
    entry: &Map<String, Value>,
    key: &str,
    accepted: &[&str],
    required: bool,
) -> bool {
    match entry.get(key) {
        Some(Value::Null) if !required => true,
        Some(value) => value
            .as_str()
            .is_some_and(|value| accepted.contains(&value)),
        None => !required,
    }
}

pub(super) fn valid_reasoning_level(value: &Value) -> bool {
    value.as_object().is_some_and(|level| {
        level
            .get("effort")
            .and_then(Value::as_str)
            .is_some_and(|effort| {
                !effort.is_empty() && effort.len() <= 64 && !effort.chars().any(char::is_control)
            })
            && required_string(level, "description")
    })
}

pub(super) fn default_service_tiers(entry: &Map<String, Value>) -> bool {
    entry.get("service_tiers").is_none_or(|value| {
        value.as_array().is_some_and(|tiers| {
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

pub(super) fn optional_message_object(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(|value| {
        value.is_null()
            || value
                .as_object()
                .is_some_and(|value| required_string(value, "message"))
    })
}

pub(super) fn optional_upgrade(entry: &Map<String, Value>) -> bool {
    entry.get("upgrade").is_none_or(|value| {
        value.is_null()
            || value.as_object().is_some_and(|upgrade| {
                required_string(upgrade, "model") && required_string(upgrade, "migration_markdown")
            })
    })
}

pub(super) fn optional_model_messages(entry: &Map<String, Value>) -> bool {
    entry.get("model_messages").is_none_or(|value| {
        value.is_null()
            || value.as_object().is_some_and(|messages| {
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

pub(super) fn optional_string_map(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(|value| {
        value.is_null()
            || value.as_object().is_some_and(|values| {
                values
                    .values()
                    .all(|value| value.is_null() || value.is_string())
            })
    })
}

pub(super) fn optional_token_budget(entry: &Map<String, Value>) -> bool {
    entry.get("token_budget").is_none_or(|value| {
        value.is_null()
            || value.as_object().is_some_and(|budget| {
                required_i64(budget, "reminder_threshold_tokens")
                    && required_string(budget, "reminder_message_template")
                    && required_string(budget, "guidance_message")
                    && required_string(budget, "auto_compact_fallback_prompt")
                    && required_i64(budget, "auto_compact_fallback_buffer_tokens")
            })
    })
}

pub(super) fn required_i64(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).and_then(Value::as_i64).is_some()
}

pub(in crate::catalog::codex) fn valid_truncation_policy(entry: &Map<String, Value>) -> bool {
    // The absence of this field is intentional for routed/API models: Relay
    // must not impose a synthetic request-size limit on an upstream that may
    // support a larger context. If a provider-owned catalog supplies the
    // policy, still validate it strictly instead of accepting malformed data.
    entry.get("truncation_policy").is_some_and(|value| {
        value.as_object().is_some_and(|policy| {
            enum_string(policy, "mode", &["bytes", "tokens"], true) && required_i64(policy, "limit")
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
    let raw = UPSTREAM_REASONING_LEVEL_KEYS
        .iter()
        .find_map(|key| template.get(*key))?;
    let levels = raw.as_array()?;
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

pub(super) fn valid_reasoning_effort_text(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
}

pub(super) fn valid_input_modalities(entry: &Map<String, Value>) -> bool {
    entry.get("input_modalities").is_none_or(|value| {
        value.as_array().is_some_and(|modalities| {
            modalities.iter().all(|modality| {
                modality
                    .as_str()
                    .is_some_and(|value| matches!(value, "text" | "image" | "audio"))
            })
        })
    })
}
