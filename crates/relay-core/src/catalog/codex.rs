use super::context::context_window;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Map, Value};

pub const CODEX_RELAY_ALIAS_PREFIX: &str = "zenith/";
pub const CODEX_RELAY_CATALOG_HASH: &str = "zenith-relay";
pub const CODEX_CATALOG_PRIORITY_BASE: u64 = 1_000;
const CODEX_RELAY_FALLBACK_CONTEXT_WINDOW: u64 = 272_000;

pub fn codex_model_alias(model: &str) -> String {
    format!(
        "{CODEX_RELAY_ALIAS_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(model.as_bytes())
    )
}

pub fn decode_codex_model_alias(alias: &str) -> Option<String> {
    let encoded = alias.strip_prefix(CODEX_RELAY_ALIAS_PREFIX)?;
    let decoded = URL_SAFE_NO_PAD.decode(encoded).ok()?;
    let model = String::from_utf8(decoded).ok()?;
    valid_model_id(&model).then_some(model)
}

pub fn codex_model_is_picker_eligible(model: &str) -> bool {
    if !valid_model_id(model) {
        return false;
    }
    let id = model.to_ascii_lowercase();
    ![
        "image",
        "audio",
        "realtime",
        "embedding",
        "moderation",
        "transcri",
        "whisper",
        "dall-e",
        "sora",
        "-tts",
    ]
    .iter()
    .any(|marker| id.contains(marker))
}

pub fn codex_model_display_name(model: &str) -> String {
    let leaf = model.rsplit('/').next().unwrap_or(model).trim();
    let mut output = String::new();
    let mut previous_was_number = false;
    for raw in leaf.split(['-', '_']).filter(|part| !part.is_empty()) {
        let number = raw.bytes().all(|byte| byte.is_ascii_digit());
        if !output.is_empty() {
            output.push(if number && previous_was_number {
                '.'
            } else {
                ' '
            });
        }
        output.push_str(&display_word(raw));
        previous_was_number = number;
    }
    if output.is_empty() {
        model.to_string()
    } else {
        output
    }
}

pub fn routed_codex_catalog_entry(
    template: Option<&Map<String, Value>>,
    model: &str,
    priority: u64,
    advertised_context_window: Option<u64>,
) -> Value {
    let mut entry = template.cloned().unwrap_or_default();
    for key in [
        "availability_nux",
        "model_messages",
        "supports_websockets",
        "upgrade",
        "use_responses_lite",
        "tool_mode",
        "multi_agent_version",
        "auto_review_model_override",
        "additional_speed_tiers",
        "service_tiers",
        "default_service_tier",
        "service_tier",
    ] {
        entry.remove(key);
    }

    entry.insert("slug".into(), Value::String(codex_model_alias(model)));
    entry.insert(
        "display_name".into(),
        Value::String(codex_model_display_name(model)),
    );
    entry.insert(
        "description".into(),
        Value::String("Available through Zenith Relay.".into()),
    );
    entry.insert("owned_by".into(), Value::String("zenith-relay".into()));
    entry.insert("shell_type".into(), Value::String("shell_command".into()));
    entry.insert("visibility".into(), Value::String("list".into()));
    entry.insert("supported_in_api".into(), Value::Bool(true));
    entry.insert(
        "priority".into(),
        Value::Number(priority.min(i32::MAX as u64).into()),
    );
    entry.insert(
        "base_instructions".into(),
        Value::String(
            "You are a coding agent. Follow the user's instructions and use the available tools."
                .into(),
        ),
    );
    entry.remove("default_reasoning_level");
    entry.insert("supported_reasoning_levels".into(), json!([]));
    entry.insert(
        "default_reasoning_summary".into(),
        Value::String("none".into()),
    );
    entry.insert(
        "supports_reasoning_summary_parameter".into(),
        Value::Bool(false),
    );
    entry.insert("supports_reasoning_summaries".into(), Value::Bool(false));
    entry.insert(
        "include_skills_usage_instructions".into(),
        Value::Bool(false),
    );
    entry.insert("support_verbosity".into(), Value::Bool(false));
    entry.insert("default_verbosity".into(), Value::Null);
    // A generic OpenAI-compatible `/v1/models` response does not prove that the
    // selected upstream accepts concurrent function calls.  Codex still receives
    // and can use ordinary tools; this only keeps it from sending more than one
    // call in a turn until a structured upstream Codex catalog proves otherwise.
    entry.insert("supports_parallel_tool_calls".into(), Value::Bool(false));
    entry.insert("supports_search_tool".into(), Value::Bool(false));
    entry.insert("web_search_tool_type".into(), Value::String("text".into()));
    entry.insert("supports_image_detail_original".into(), Value::Bool(false));
    // Codex uses this field as a client-side attachment gate. Routed sources
    // may omit or under-report vision metadata, so let the selected upstream
    // model make the final capability decision instead of blocking the image
    // before Relay receives it.
    entry.insert("input_modalities".into(), json!(["text", "image"]));
    entry.insert("experimental_supported_tools".into(), json!([]));
    entry.insert(
        "apply_patch_tool_type".into(),
        Value::String("freeform".into()),
    );
    entry.insert(
        "truncation_policy".into(),
        json!({ "mode": "tokens", "limit": 10_000 }),
    );
    let context_window = advertised_context_window
        .or_else(|| entry.get("context_window").and_then(Value::as_u64))
        .filter(|window| *window > 0)
        .unwrap_or(CODEX_RELAY_FALLBACK_CONTEXT_WINDOW);
    entry.insert("context_window".into(), context_window.into());
    if advertised_context_window.is_some() {
        entry.insert("max_context_window".into(), context_window.into());
    } else {
        entry.remove("max_context_window");
    }
    entry.remove("auto_compact_token_limit");
    entry.insert("effective_context_window_percent".into(), 95.into());
    entry.insert(
        "comp_hash".into(),
        Value::String(CODEX_RELAY_CATALOG_HASH.into()),
    );
    Value::Object(entry)
}

/// Codex uses the numeric priority as the picker sort key. Keep it unique
/// after combining native upstream rows with Relay-generated fallback rows.
pub fn normalize_codex_catalog_priorities(models: &mut [Value]) {
    for (index, model) in models.iter_mut().enumerate() {
        let Some(entry) = model.as_object_mut() else {
            continue;
        };
        let priority = CODEX_CATALOG_PRIORITY_BASE.saturating_add(index as u64);
        entry.insert("priority".into(), priority.into());
    }
}

pub fn normalize_upstream_codex_catalog_entry(
    template: &Map<String, Value>,
    model: &str,
    priority: u64,
    advertised_context_window: Option<u64>,
) -> Option<Value> {
    let mut entry = catalog_entry_base(template, model, priority, advertised_context_window)?;

    for key in [
        "additional_speed_tiers",
        "default_service_tier",
        "default_reasoning_level",
    ] {
        if let Some(value) = template.get(key) {
            let valid = match key {
                "additional_speed_tiers" => {
                    let mut candidate = Map::new();
                    candidate.insert(key.into(), value.clone());
                    default_string_array(&candidate, key)
                }
                _ => optional_non_empty_string(template, key),
            };
            if valid {
                entry.insert(key.into(), value.clone());
            }
        }
    }

    if let Some(value) = template.get("service_tiers") {
        let mut candidate = Map::new();
        candidate.insert("service_tiers".into(), value.clone());
        if default_service_tiers(&candidate) {
            entry.insert("service_tiers".into(), value.clone());
        }
    }

    if let Some(value) = template.get("supported_reasoning_levels") {
        if value
            .as_array()
            .is_some_and(|levels| levels.iter().all(valid_reasoning_level))
        {
            entry.insert("supported_reasoning_levels".into(), value.clone());
        }
    }
    // API routes use Relay's neutral automatic default and never inherit an
    // upstream automatic default such as `ultra`.
    prefer_medium_reasoning_default(&mut entry);

    for key in [
        "use_responses_lite",
        "supports_parallel_tool_calls",
        "supports_search_tool",
        "supports_image_detail_original",
        "include_skills_usage_instructions",
        "supports_reasoning_summary_parameter",
        "supports_reasoning_summaries",
    ] {
        if template.get(key).is_some_and(Value::is_boolean) {
            entry.insert(key.into(), template[key].clone());
        }
    }

    for key in ["default_reasoning_summary", "web_search_tool_type"] {
        let accepted = if key == "default_reasoning_summary" {
            ["auto", "concise", "detailed", "none"].as_slice()
        } else {
            ["text", "text_and_image"].as_slice()
        };
        if template
            .get(key)
            .and_then(Value::as_str)
            .is_some_and(|value| accepted.contains(&value))
        {
            entry.insert(key.into(), template[key].clone());
        }
    }

    if let Some(value) = template.get("experimental_supported_tools") {
        let mut candidate = Map::new();
        candidate.insert("experimental_supported_tools".into(), value.clone());
        if default_string_array(&candidate, "experimental_supported_tools") {
            entry.insert("experimental_supported_tools".into(), value.clone());
        }
    }
    if let Some(value) = template.get("truncation_policy") {
        let mut candidate = Map::new();
        candidate.insert("truncation_policy".into(), value.clone());
        if valid_truncation_policy(&candidate) {
            entry.insert("truncation_policy".into(), value.clone());
        }
    }

    if advertised_context_window.is_none() {
        if let Some(upstream_context_window) =
            template.get("context_window").and_then(context_window)
        {
            entry.insert("context_window".into(), upstream_context_window.into());
            if let Some(max_context_window) =
                template.get("max_context_window").and_then(context_window)
            {
                entry.insert("max_context_window".into(), max_context_window.into());
            }
        }
    }

    let value = Value::Object(entry);
    codex_catalog_entry_is_compatible(&value).then_some(value)
}

fn prefer_medium_reasoning_default(entry: &mut Map<String, Value>) {
    let Some(medium) = entry
        .get("supported_reasoning_levels")
        .and_then(Value::as_array)
        .and_then(|levels| {
            levels.iter().find_map(|level| {
                level
                    .get("effort")
                    .and_then(Value::as_str)
                    .filter(|effort| effort.eq_ignore_ascii_case("medium"))
            })
        })
        .map(str::to_owned)
    else {
        entry.remove("default_reasoning_level");
        return;
    };

    entry.insert("default_reasoning_level".into(), Value::String(medium));
}

/// Preserve the upstream Codex identity for a confirmed ChatGPT account model.
///
/// Provider-routed rows intentionally use `codex_model_alias` and the
/// conservative `routed_codex_catalog_entry` path.  A native OAuth model is
/// different: Codex uses the bare upstream slug to select its native
/// Responses contract, so replacing it with a Relay alias would hide the
/// account's native reasoning and service-tier controls.
pub fn normalize_native_codex_catalog_entry(
    template: &Map<String, Value>,
    model: &str,
    priority: u64,
    _advertised_context_window: Option<u64>,
) -> Option<Value> {
    // Native rows own their context and capability fields. An API-source
    // context override must never be allowed to fill or replace them.
    let mut entry = catalog_entry_base(template, model, priority, None)?;
    // Start from a known-compatible native-shaped row so partial manifests
    // cannot make the whole pool catalog row disappear, then overlay every
    // upstream field to retain native capabilities verbatim.
    entry.extend(
        template
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    if !template.contains_key("input_modalities") {
        entry.remove("input_modalities");
    }
    let context_window = entry.get("context_window").and_then(context_window);
    entry.insert("slug".into(), Value::String(model.to_string()));
    entry.insert(
        "priority".into(),
        Value::Number(priority.min(i32::MAX as u64).into()),
    );
    if let Some(context_window) = context_window {
        entry.insert("context_window".into(), context_window.into());
        entry
            .entry("max_context_window")
            .or_insert_with(|| context_window.into());
    }
    codex_catalog_entry_is_compatible(&Value::Object(entry.clone())).then_some(Value::Object(entry))
}

pub fn codex_catalog_entry_is_compatible(value: &Value) -> bool {
    let Some(entry) = value.as_object() else {
        return false;
    };
    required_non_empty_string(entry, "slug")
        && required_string(entry, "display_name")
        && optional_string(entry, "description")
        && optional_non_empty_string(entry, "default_reasoning_level")
        && entry
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .is_some_and(|levels| levels.iter().all(valid_reasoning_level))
        && enum_string(
            entry,
            "shell_type",
            &[
                "default",
                "local",
                "unified_exec",
                "disabled",
                "shell_command",
            ],
            true,
        )
        && enum_string(entry, "visibility", &["list", "hide", "none"], true)
        && required_bool(entry, "supported_in_api")
        && required_i32(entry, "priority")
        && default_string_array(entry, "additional_speed_tiers")
        && default_service_tiers(entry)
        && optional_string(entry, "default_service_tier")
        && optional_message_object(entry, "availability_nux")
        && optional_upgrade(entry)
        && required_string(entry, "base_instructions")
        && optional_model_messages(entry)
        && default_bool(entry, "include_skills_usage_instructions")
        && default_bool(entry, "supports_reasoning_summary_parameter")
        && default_bool(entry, "supports_reasoning_summaries")
        && default_enum_string(
            entry,
            "default_reasoning_summary",
            &["auto", "concise", "detailed", "none"],
        )
        && required_bool(entry, "support_verbosity")
        && enum_string(
            entry,
            "default_verbosity",
            &["low", "medium", "high"],
            false,
        )
        && enum_string(entry, "apply_patch_tool_type", &["freeform"], false)
        && default_enum_string(entry, "web_search_tool_type", &["text", "text_and_image"])
        && valid_truncation_policy(entry)
        && required_bool(entry, "supports_parallel_tool_calls")
        && default_bool(entry, "supports_image_detail_original")
        && optional_i64(entry, "context_window")
        && optional_i64(entry, "max_context_window")
        && optional_i64(entry, "auto_compact_token_limit")
        && optional_string(entry, "comp_hash")
        && optional_i64(entry, "effective_context_window_percent")
        && entry
            .get("experimental_supported_tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().all(Value::is_string))
        && valid_input_modalities(entry)
        && default_bool(entry, "supports_search_tool")
        && default_bool(entry, "use_responses_lite")
        && optional_string(entry, "auto_review_model_override")
        && enum_string(
            entry,
            "tool_mode",
            &["direct", "code_mode", "code_mode_only"],
            false,
        )
        && enum_string(
            entry,
            "multi_agent_version",
            &["disabled", "v1", "v2"],
            false,
        )
}

fn required_string(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_some_and(Value::is_string)
}

fn required_non_empty_string(entry: &Map<String, Value>, key: &str) -> bool {
    entry
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty() && !value.chars().any(char::is_control))
}

fn optional_string(entry: &Map<String, Value>, key: &str) -> bool {
    entry
        .get(key)
        .is_none_or(|value| value.is_null() || value.is_string())
}

fn optional_non_empty_string(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(|value| {
        value.is_null()
            || value
                .as_str()
                .is_some_and(|value| !value.is_empty() && !value.chars().any(char::is_control))
    })
}

fn required_bool(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_some_and(Value::is_boolean)
}

fn default_bool(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(Value::is_boolean)
}

fn required_i32(entry: &Map<String, Value>, key: &str) -> bool {
    entry
        .get(key)
        .and_then(Value::as_i64)
        .is_some_and(|value| i32::try_from(value).is_ok())
}

fn optional_i64(entry: &Map<String, Value>, key: &str) -> bool {
    entry
        .get(key)
        .is_none_or(|value| value.is_null() || value.as_i64().is_some())
}

fn default_string_array(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(|value| {
        value
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string))
    })
}

fn default_enum_string(entry: &Map<String, Value>, key: &str, accepted: &[&str]) -> bool {
    entry.get(key).is_none_or(|value| {
        value
            .as_str()
            .is_some_and(|value| accepted.contains(&value))
    })
}

fn enum_string(entry: &Map<String, Value>, key: &str, accepted: &[&str], required: bool) -> bool {
    match entry.get(key) {
        Some(Value::Null) if !required => true,
        Some(value) => value
            .as_str()
            .is_some_and(|value| accepted.contains(&value)),
        None => !required,
    }
}

fn valid_reasoning_level(value: &Value) -> bool {
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

fn default_service_tiers(entry: &Map<String, Value>) -> bool {
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

fn optional_message_object(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(|value| {
        value.is_null()
            || value
                .as_object()
                .is_some_and(|value| required_string(value, "message"))
    })
}

fn optional_upgrade(entry: &Map<String, Value>) -> bool {
    entry.get("upgrade").is_none_or(|value| {
        value.is_null()
            || value.as_object().is_some_and(|upgrade| {
                required_string(upgrade, "model") && required_string(upgrade, "migration_markdown")
            })
    })
}

fn optional_model_messages(entry: &Map<String, Value>) -> bool {
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

fn optional_string_map(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).is_none_or(|value| {
        value.is_null()
            || value.as_object().is_some_and(|values| {
                values
                    .values()
                    .all(|value| value.is_null() || value.is_string())
            })
    })
}

fn optional_token_budget(entry: &Map<String, Value>) -> bool {
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

fn required_i64(entry: &Map<String, Value>, key: &str) -> bool {
    entry.get(key).and_then(Value::as_i64).is_some()
}

fn valid_truncation_policy(entry: &Map<String, Value>) -> bool {
    entry
        .get("truncation_policy")
        .and_then(Value::as_object)
        .is_some_and(|policy| {
            enum_string(policy, "mode", &["bytes", "tokens"], true) && required_i64(policy, "limit")
        })
}

fn valid_input_modalities(entry: &Map<String, Value>) -> bool {
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

fn catalog_entry_base(
    template: &Map<String, Value>,
    model: &str,
    priority: u64,
    advertised_context_window: Option<u64>,
) -> Option<Map<String, Value>> {
    routed_codex_catalog_entry(
        None,
        model,
        priority,
        advertised_context_window
            .or_else(|| template.get("context_window").and_then(context_window)),
    )
    .as_object()
    .cloned()
}

fn valid_model_id(model: &str) -> bool {
    !model.trim().is_empty()
        && model.len() <= 256
        && model.trim() == model
        && !model.chars().any(char::is_control)
}

fn display_word(word: &str) -> String {
    match word.to_ascii_lowercase().as_str() {
        "gpt" => "GPT".into(),
        "glm" => "GLM".into(),
        "xai" => "xAI".into(),
        "qwen" => "Qwen".into(),
        "deepseek" => "DeepSeek".into(),
        "claude" => "Claude".into(),
        "gemini" => "Gemini".into(),
        "grok" => "Grok".into(),
        "codex" => "Codex".into(),
        _ => {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect())
                .unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_aliases_are_exact_and_media_models_stay_out_of_codex() {
        let model = "vendor/claude-opus-4-8";
        let alias = codex_model_alias(model);
        assert_eq!(decode_codex_model_alias(&alias).as_deref(), Some(model));
        assert_eq!(codex_model_display_name(model), "Claude Opus 4.8");
        assert!(codex_model_is_picker_eligible(model));
        assert!(!codex_model_is_picker_eligible("gpt-image-2"));
        assert!(decode_codex_model_alias("zenith/not-base64!").is_none());
    }

    #[test]
    fn generated_picker_order_matches_relay_provider_groups() {
        let models = crate::canonicalize_model_ids([
            "vendor/glm-5.2",
            "vendor/grok-4.5",
            "vendor/gemini-3.6-flash",
            "vendor/claude-opus-4-8",
            "gpt-5.4-mini",
            "vendor/gpt-5.4",
            "gpt-5.5",
            "gpt-5.6-luna",
            "gpt-5.6-terra",
            "gpt-5.6-sol",
            "vendor/unknown-model",
        ]);

        assert_eq!(
            models,
            [
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.6-luna",
                "gpt-5.5",
                "vendor/gpt-5.4",
                "gpt-5.4-mini",
                "vendor/claude-opus-4-8",
                "vendor/gemini-3.6-flash",
                "vendor/glm-5.2",
                "vendor/grok-4.5",
                "vendor/unknown-model",
            ]
        );
    }

    #[test]
    fn catalog_aliases_do_not_change_the_model_identity() {
        let alias = codex_model_alias("gpt-5.6-sol");
        assert_eq!(
            decode_codex_model_alias(&alias).as_deref(),
            Some("gpt-5.6-sol")
        );
    }

    #[test]
    fn routed_models_strip_native_only_selectors_from_template() {
        let template = json!({
            "base_instructions": "native Codex instructions",
            "model_messages": {"instructions_template": "native template"},
            "tool_mode": "code_mode",
            "multi_agent_version": "v2",
            "default_reasoning_level": "medium",
            "supported_reasoning_levels": [{"effort": "low", "description": "Low"}],
            "web_search_tool_type": "text_and_image",
            "use_responses_lite": true,
        });
        let entry =
            routed_codex_catalog_entry(template.as_object(), "vendor/claude-fable-5", 1_000, None);

        assert_eq!(
            entry["base_instructions"],
            "You are a coding agent. Follow the user's instructions and use the available tools."
        );
        assert!(entry.get("model_messages").is_none());
        assert!(entry.get("tool_mode").is_none());
        assert!(entry.get("multi_agent_version").is_none());
        assert!(entry.get("default_reasoning_level").is_none());
        assert_eq!(entry["supported_reasoning_levels"], json!([]));
        assert_eq!(entry["web_search_tool_type"], "text");
        assert!(entry.get("use_responses_lite").is_none());
        assert!(entry.get("service_tiers").is_none());
        assert_eq!(entry["supports_reasoning_summaries"], false);
        assert_eq!(entry["supports_parallel_tool_calls"], false);
        assert_eq!(entry["input_modalities"], json!(["text", "image"]));
    }

    #[test]
    fn api_models_use_medium_when_provider_default_is_ultra() {
        let template = json!({
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low", "description": "Low"},
                {"effort": "medium", "description": "Medium"},
                {"effort": "ultra", "description": "Ultra"}
            ]
        });

        let entry = normalize_upstream_codex_catalog_entry(
            template.as_object().unwrap(),
            "vendor/model",
            1_000,
            None,
        )
        .expect("API catalog entry");

        assert_eq!(entry["default_reasoning_level"], "medium");
        assert_eq!(
            entry["supported_reasoning_levels"]
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn api_models_do_not_inherit_ultra_when_medium_is_unavailable() {
        let template = json!({
            "default_reasoning_level": "ultra",
            "supported_reasoning_levels": [
                {"effort": "low", "description": "Low"},
                {"effort": "high", "description": "High"},
                {"effort": "ultra", "description": "Ultra"}
            ]
        });

        let entry = normalize_upstream_codex_catalog_entry(
            template.as_object().unwrap(),
            "vendor/model",
            1_000,
            None,
        )
        .expect("API catalog entry");

        assert!(entry.get("default_reasoning_level").is_none());
    }

    #[test]
    fn native_models_keep_bare_slug_and_upstream_capabilities() {
        let template = json!({
            "slug": "gpt-5.6-sol",
            "display_name": "GPT-5.6 Sol",
            "base_instructions": "native Codex instructions",
            "shell_type": "default",
            "visibility": "list",
            "supported_in_api": true,
            "priority": 10,
            "default_reasoning_level": "high",
            "supported_reasoning_levels": [{"effort": "low", "description": "Low"}],
            "service_tiers": [{
                "id": "priority",
                "name": "Fast",
                "description": "Native fast tier"
            }],
            "default_service_tier": "priority",
            "additional_speed_tiers": ["priority"],
            "supports_reasoning_summary_parameter": true,
            "supports_reasoning_summaries": true,
            "default_reasoning_summary": "detailed",
            "support_verbosity": true,
            "default_verbosity": "medium",
            "supports_parallel_tool_calls": true,
            "supports_image_detail_original": true,
            "supports_search_tool": true,
            "use_responses_lite": true,
            "input_modalities": ["text"],
            "experimental_supported_tools": [],
            "apply_patch_tool_type": "freeform",
            "truncation_policy": {"mode": "tokens", "limit": 10000},
            "context_window": 128000,
            "max_context_window": 120000,
            "auto_compact_token_limit": 110000,
            "native_setting": "keep-me",
        });
        let entry = normalize_native_codex_catalog_entry(
            template.as_object().unwrap(),
            "gpt-5.6-sol",
            1_000,
            Some(1_000_000),
        )
        .unwrap();

        assert_eq!(entry["slug"], "gpt-5.6-sol");
        assert_eq!(entry["default_reasoning_level"], "high");
        assert_eq!(entry["service_tiers"][0]["id"], "priority");
        assert_eq!(entry["supports_parallel_tool_calls"], true);
        assert_eq!(entry["use_responses_lite"], true);
        assert_eq!(entry["input_modalities"], json!(["text"]));
        assert_eq!(entry["context_window"], 128_000);
        assert_eq!(entry["max_context_window"], 120_000);
        assert_eq!(entry["auto_compact_token_limit"], 110_000);
        assert_eq!(entry["native_setting"], "keep-me");
    }

    #[test]
    fn native_models_do_not_inherit_routed_image_defaults() {
        let template = json!({
            "slug": "gpt-native",
            "display_name": "GPT Native",
        });

        let entry = normalize_native_codex_catalog_entry(
            template.as_object().unwrap(),
            "gpt-native",
            1_000,
            None,
        )
        .unwrap();

        assert!(entry.get("input_modalities").is_none());
    }

    #[test]
    fn routed_models_use_advertised_context_and_do_not_clamp_unknown_overrides() {
        let template = json!({
            "context_window": 272_000,
            "max_context_window": 272_000,
            "auto_compact_token_limit": 244_800,
        });

        let advertised = routed_codex_catalog_entry(
            template.as_object(),
            "vendor/large",
            1_000,
            Some(1_000_000),
        );
        assert_eq!(advertised["context_window"], 1_000_000);
        assert_eq!(advertised["max_context_window"], 1_000_000);
        assert!(advertised.get("auto_compact_token_limit").is_none());

        let unknown =
            routed_codex_catalog_entry(template.as_object(), "vendor/unknown", 1_001, None);
        assert_eq!(unknown["context_window"], 272_000);
        assert!(unknown.get("max_context_window").is_none());
        assert!(unknown.get("auto_compact_token_limit").is_none());
    }

    #[test]
    fn strict_catalog_validation_rejects_poisoned_or_incomplete_rows() {
        let valid = routed_codex_catalog_entry(None, "vendor/model", 1_000, None);
        assert!(codex_catalog_entry_is_compatible(&valid));

        let mut missing_required = valid.clone();
        missing_required
            .as_object_mut()
            .unwrap()
            .remove("supported_reasoning_levels");
        assert!(!codex_catalog_entry_is_compatible(&missing_required));

        let mut poisoned = valid;
        poisoned["input_modalities"] = json!(["text", "video"]);
        assert!(!codex_catalog_entry_is_compatible(&poisoned));
    }
}
