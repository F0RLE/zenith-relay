use super::super::context::context_window;
use super::{routed_codex_catalog_entry, set_codex_service_tiers};
use serde_json::{Map, Value};

mod checks;

pub use checks::source_row_declares_reasoning;
use checks::{
    default_bool, default_enum_string, default_service_tiers, enum_string, optional_i64,
    optional_message_object, optional_model_messages, optional_string, optional_upgrade,
    required_bool, required_i32, required_non_empty_string, required_string,
    valid_input_modalities, valid_reasoning_level,
};
pub(super) use checks::{
    default_string_array, optional_non_empty_string, upstream_reasoning_levels,
    valid_truncation_policy,
};

/// Codex renders the reasoning description below the effort label, but its
/// picker reserves a single compact row for each option. Provider catalogs
/// often send prose descriptions that wrap into the following option and make
/// the menu unreadable. Keep short labels intact and collapse verbose copy to
/// the stable effort identifier; this changes presentation only, not support.
pub(super) fn compact_reasoning_level_descriptions(catalog_entry: &mut Map<String, Value>) {
    let Some(levels) = catalog_entry
        .get_mut("supported_reasoning_levels")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    for level in levels {
        let Some(level) = level.as_object_mut() else {
            continue;
        };
        let Some(effort) = level
            .get("effort")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|effort| !effort.is_empty())
            .map(str::to_owned)
        else {
            continue;
        };
        let verbose = level
            .get("description")
            .and_then(Value::as_str)
            .is_some_and(|description| {
                description.chars().count() > 24 || description.contains(['\n', '\r'])
            });
        if verbose {
            level.insert("description".into(), Value::String(effort));
        }
    }
}

pub(super) fn prefer_medium_reasoning_default(catalog_entry: &mut Map<String, Value>) {
    let Some(medium) = catalog_entry
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
        catalog_entry.remove("default_reasoning_level");
        return;
    };

    catalog_entry.insert("default_reasoning_level".into(), Value::String(medium));
}

/// Preserve the upstream Codex identity for a confirmed ChatGPT account model.
///
/// Provider-routed rows intentionally use `codex_model_alias` and the
/// conservative `routed_codex_catalog_entry` path.  A native OAuth model is
/// different: Codex uses the bare upstream slug to select its native
/// Responses contract. Semantic model fields are overlaid from the shared
/// reference catalog by the caller; transport remains account-owned.
pub fn normalize_native_codex_catalog_entry(
    template: &Map<String, Value>,
    model: &str,
    priority: u64,
    _advertised_context_window: Option<u64>,
) -> Option<Value> {
    // Normalize the account transport template without an API context override.
    // The final projection applies shared model semantics and client context policy.
    let mut catalog_entry = catalog_entry_base(template, model, priority, None)?;
    // `catalog_entry_base` starts from the routed fallback schema, which has
    // a context value for API clients. Native catalogs are different: an
    // omitted field means Codex owns the context policy, so do not manufacture
    // a Relay limit before overlaying the native row.
    for key in [
        "context_window",
        "max_context_window",
        "auto_compact_token_limit",
        "effective_context_window_percent",
    ] {
        catalog_entry.remove(key);
    }
    // Start from a known-compatible native-shaped row so partial manifests
    // cannot make the whole pool catalog row disappear, then overlay every
    // upstream field to retain native capabilities. Speed is Relay-owned.
    catalog_entry.extend(
        template
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    if !template.contains_key("input_modalities") {
        catalog_entry.remove("input_modalities");
    }
    // Native account rows are identified by the upstream slug. The caller's
    // model is only a routing fallback; never replace a real upstream ID with
    // a Relay alias or a configured spelling.
    let native_model = template
        .get("slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|slug| valid_model_id(slug))
        .unwrap_or(model);
    catalog_entry.insert("slug".into(), Value::String(native_model.to_string()));
    catalog_entry.insert(
        "priority".into(),
        Value::Number(priority.min(i32::MAX as u64).into()),
    );
    let mut catalog_value = Value::Object(catalog_entry);
    set_codex_service_tiers(
        &mut catalog_value,
        super::super::model_service_tiers(model, None),
    );
    codex_catalog_entry_is_compatible(&catalog_value).then_some(catalog_value)
}

pub fn codex_catalog_entry_is_compatible(catalog_entry_value: &Value) -> bool {
    let Some(catalog_entry) = catalog_entry_value.as_object() else {
        return false;
    };
    required_non_empty_string(catalog_entry, "slug")
        && required_string(catalog_entry, "display_name")
        && optional_string(catalog_entry, "description")
        && optional_non_empty_string(catalog_entry, "default_reasoning_level")
        && catalog_entry
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .is_some_and(|levels| levels.iter().all(valid_reasoning_level))
        && enum_string(
            catalog_entry,
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
        && enum_string(catalog_entry, "visibility", &["list", "hide", "none"], true)
        && required_bool(catalog_entry, "supported_in_api")
        && required_i32(catalog_entry, "priority")
        && default_string_array(catalog_entry, "additional_speed_tiers")
        && default_service_tiers(catalog_entry)
        && optional_string(catalog_entry, "default_service_tier")
        && optional_message_object(catalog_entry, "availability_nux")
        && optional_upgrade(catalog_entry)
        && required_string(catalog_entry, "base_instructions")
        && optional_model_messages(catalog_entry)
        && default_bool(catalog_entry, "include_skills_usage_instructions")
        && default_bool(catalog_entry, "supports_reasoning_summary_parameter")
        && default_bool(catalog_entry, "supports_reasoning_summaries")
        && default_enum_string(
            catalog_entry,
            "default_reasoning_summary",
            &["auto", "concise", "detailed", "none"],
        )
        && required_bool(catalog_entry, "support_verbosity")
        && enum_string(
            catalog_entry,
            "default_verbosity",
            &["low", "medium", "high"],
            false,
        )
        && enum_string(catalog_entry, "apply_patch_tool_type", &["freeform"], false)
        && default_enum_string(
            catalog_entry,
            "web_search_tool_type",
            &["text", "text_and_image"],
        )
        && valid_truncation_policy(catalog_entry)
        && required_bool(catalog_entry, "supports_parallel_tool_calls")
        && default_bool(catalog_entry, "supports_image_detail_original")
        && optional_i64(catalog_entry, "context_window")
        && optional_i64(catalog_entry, "max_context_window")
        && optional_i64(catalog_entry, "auto_compact_token_limit")
        && optional_string(catalog_entry, "comp_hash")
        && optional_i64(catalog_entry, "effective_context_window_percent")
        && catalog_entry
            .get("experimental_supported_tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().all(Value::is_string))
        && valid_input_modalities(catalog_entry)
        && default_bool(catalog_entry, "supports_search_tool")
        && default_bool(catalog_entry, "use_responses_lite")
        && optional_string(catalog_entry, "auto_review_model_override")
        && enum_string(
            catalog_entry,
            "tool_mode",
            &["direct", "code_mode", "code_mode_only"],
            false,
        )
        && enum_string(
            catalog_entry,
            "multi_agent_version",
            &["disabled", "v1", "v2"],
            false,
        )
}

pub(super) fn catalog_entry_base(
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

pub(super) fn valid_model_id(model: &str) -> bool {
    !model.trim().is_empty()
        && model.len() <= 256
        && model.trim() == model
        && !model.chars().any(char::is_control)
}

pub(super) fn display_word(word: &str) -> String {
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
