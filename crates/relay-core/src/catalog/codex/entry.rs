use super::super::context::context_window;
use super::shape::{
    catalog_entry_base, compact_reasoning_level_descriptions, default_string_array,
    optional_non_empty_string, prefer_medium_reasoning_default, upstream_reasoning_levels,
    valid_truncation_policy,
};
use super::{
    codex_catalog_entry_is_compatible, codex_model_alias, codex_model_display_name,
    set_codex_service_tiers, CODEX_CATALOG_PRIORITY_BASE, CODEX_RELAY_CATALOG_HASH,
    CODEX_RELAY_FALLBACK_CONTEXT_WINDOW,
};
use serde_json::{json, Map, Value};

pub(super) const ROUTED_CODEX_BASE_INSTRUCTIONS: &str = concat!(
    "You are a coding agent. Follow the user's instructions and use the available tools. ",
    "Edit files with apply_patch, not the shell or Python. ",
    "Run commands in PowerShell on Windows, and in the user's shell on macOS and Linux."
);

pub fn routed_codex_catalog_entry(
    template: Option<&Map<String, Value>>,
    model: &str,
    priority: u64,
    advertised_context_window: Option<u64>,
) -> Value {
    let mut catalog_entry = template.cloned().unwrap_or_default();
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
        catalog_entry.remove(key);
    }

    catalog_entry.insert("slug".into(), Value::String(codex_model_alias(model)));
    catalog_entry.insert(
        "display_name".into(),
        Value::String(codex_model_display_name(model)),
    );
    catalog_entry.insert(
        "description".into(),
        Value::String("Available through Zenith Relay.".into()),
    );
    catalog_entry.insert("owned_by".into(), Value::String("zenith-relay".into()));
    catalog_entry.insert("shell_type".into(), Value::String("shell_command".into()));
    catalog_entry.insert("visibility".into(), Value::String("list".into()));
    catalog_entry.insert("supported_in_api".into(), Value::Bool(true));
    catalog_entry.insert(
        "priority".into(),
        Value::Number(priority.min(i32::MAX as u64).into()),
    );
    catalog_entry.insert(
        "base_instructions".into(),
        Value::String(ROUTED_CODEX_BASE_INSTRUCTIONS.into()),
    );
    catalog_entry.remove("default_reasoning_level");
    catalog_entry.insert("supported_reasoning_levels".into(), json!([]));
    catalog_entry.insert(
        "default_reasoning_summary".into(),
        Value::String("none".into()),
    );
    catalog_entry.insert(
        "supports_reasoning_summary_parameter".into(),
        Value::Bool(false),
    );
    catalog_entry.insert("supports_reasoning_summaries".into(), Value::Bool(false));
    catalog_entry.insert(
        "include_skills_usage_instructions".into(),
        Value::Bool(false),
    );
    catalog_entry.insert("support_verbosity".into(), Value::Bool(false));
    catalog_entry.insert("default_verbosity".into(), Value::Null);
    // A generic OpenAI-compatible `/v1/models` response does not prove that the
    // selected upstream accepts concurrent function calls.  Codex still receives
    // and can use ordinary tools; this only keeps it from sending more than one
    // call in a turn until a structured upstream Codex catalog proves otherwise.
    catalog_entry.insert("supports_parallel_tool_calls".into(), Value::Bool(false));
    catalog_entry.insert("supports_search_tool".into(), Value::Bool(false));
    catalog_entry.insert("web_search_tool_type".into(), Value::String("text".into()));
    catalog_entry.insert("supports_image_detail_original".into(), Value::Bool(false));
    // Unknown models support text/image input; models.dev replaces this
    // fallback for known exact model identities at the publication boundary.
    catalog_entry.insert("input_modalities".into(), json!(["text", "image"]));
    catalog_entry.insert("experimental_supported_tools".into(), json!([]));
    catalog_entry.insert(
        "apply_patch_tool_type".into(),
        Value::String("freeform".into()),
    );
    // Codex currently requires a truncation policy in every catalog entry.
    // Keep this as a small client-side parsing default; it does not describe
    // provider capabilities or change Relay's route selection.
    catalog_entry.insert(
        "truncation_policy".into(),
        json!({"mode": "tokens", "limit": 10000}),
    );
    let context_window = advertised_context_window
        .or_else(|| catalog_entry.get("context_window").and_then(Value::as_u64))
        .filter(|window| *window > 0)
        .unwrap_or(CODEX_RELAY_FALLBACK_CONTEXT_WINDOW);
    catalog_entry.insert("context_window".into(), context_window.into());
    if advertised_context_window.is_some() {
        catalog_entry.insert("max_context_window".into(), context_window.into());
    } else {
        catalog_entry.remove("max_context_window");
    }
    // Do not synthesize auto-compaction metadata for routed models. The
    // provider owns that policy, and publishing a Relay-side limit would make
    // the catalog claim a capability that was never observed upstream.
    catalog_entry.remove("auto_compact_token_limit");
    catalog_entry.insert("effective_context_window_percent".into(), 95.into());
    catalog_entry.insert(
        "comp_hash".into(),
        Value::String(CODEX_RELAY_CATALOG_HASH.into()),
    );
    Value::Object(catalog_entry)
}

/// Codex uses the numeric priority as the picker sort key. Keep it unique
/// after combining native upstream rows with Relay-generated fallback rows.
pub fn normalize_codex_catalog_priorities(models: &mut [Value]) {
    for (index, model) in models.iter_mut().enumerate() {
        let Some(catalog_entry) = model.as_object_mut() else {
            continue;
        };
        let priority = CODEX_CATALOG_PRIORITY_BASE.saturating_add(index as u64);
        catalog_entry.insert("priority".into(), priority.into());
    }
}

/// Ultra is a Codex orchestration mode, not a provider reasoning effort. Only
/// an exact Codex-owned model card may enable it, and both the parent (Max)
/// and the configured subagent effort must already work through this route.
/// Never copy transport capabilities or instructions from the reference card.
pub fn apply_codex_ultra_from_official_model(
    catalog_entry: &mut Value,
    official_model: &Value,
    upstream_model: &str,
) -> bool {
    if official_model
        .get("slug")
        .and_then(Value::as_str)
        .is_none_or(|slug| !slug.eq_ignore_ascii_case(upstream_model))
        || !official_model
            .get("supported_reasoning_levels")
            .and_then(Value::as_array)
            .is_some_and(|levels| {
                levels
                    .iter()
                    .any(|level| level.get("effort").and_then(Value::as_str) == Some("ultra"))
            })
    {
        return false;
    }
    let Some(levels) = catalog_entry
        .get_mut("supported_reasoning_levels")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    let supports = |effort: &str| {
        levels.iter().any(|level| {
            level
                .get("effort")
                .and_then(Value::as_str)
                .is_some_and(|candidate| candidate.eq_ignore_ascii_case(effort))
        })
    };
    let subagent_effort = official_model
        .get("multi_agent_reasoning_effort")
        .and_then(Value::as_str);
    if !supports("max") || subagent_effort.is_some_and(|effort| !supports(effort)) {
        return false;
    }
    if !supports("ultra") {
        levels.push(json!({"effort": "ultra", "description": "Ultra (agents)"}));
    }
    if let Some(version) = official_model
        .get("multi_agent_version")
        .and_then(Value::as_str)
        .filter(|version| matches!(*version, "v1" | "v2"))
    {
        catalog_entry["multi_agent_version"] = json!(version);
    }
    if let Some(effort) = subagent_effort {
        catalog_entry["multi_agent_reasoning_effort"] = json!(effort);
    }
    true
}

pub fn normalize_upstream_codex_catalog_entry(
    template: &Map<String, Value>,
    model: &str,
    priority: u64,
    advertised_context_window: Option<u64>,
) -> Option<Value> {
    let mut catalog_entry =
        catalog_entry_base(template, model, priority, advertised_context_window)?;

    if let Some(image_input) = super::super::source_model_declares_image_input(template) {
        catalog_entry.insert(
            "input_modalities".into(),
            if image_input {
                json!(["text", "image"])
            } else {
                json!(["text"])
            },
        );
    }

    if let Some(default_reasoning_level) = template.get("default_reasoning_level") {
        if optional_non_empty_string(template, "default_reasoning_level") {
            catalog_entry.insert(
                "default_reasoning_level".into(),
                default_reasoning_level.clone(),
            );
        }
    }

    if let Some(reasoning_levels) = upstream_reasoning_levels(template) {
        catalog_entry.insert("supported_reasoning_levels".into(), reasoning_levels);
        compact_reasoning_level_descriptions(&mut catalog_entry);
    }
    // API routes use Relay's neutral automatic default and never inherit an
    // upstream automatic default such as `ultra`.
    prefer_medium_reasoning_default(&mut catalog_entry);

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
            catalog_entry.insert(key.into(), template[key].clone());
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
            .is_some_and(|parameter_value| accepted.contains(&parameter_value))
        {
            catalog_entry.insert(key.into(), template[key].clone());
        }
    }

    if let Some(supported_tools) = template.get("experimental_supported_tools") {
        let mut candidate = Map::new();
        candidate.insert(
            "experimental_supported_tools".into(),
            supported_tools.clone(),
        );
        if default_string_array(&candidate, "experimental_supported_tools") {
            catalog_entry.insert(
                "experimental_supported_tools".into(),
                supported_tools.clone(),
            );
        }
    }
    if let Some(truncation_policy) = template.get("truncation_policy") {
        let mut candidate = Map::new();
        candidate.insert("truncation_policy".into(), truncation_policy.clone());
        if valid_truncation_policy(&candidate) {
            catalog_entry.insert("truncation_policy".into(), truncation_policy.clone());
        }
    }

    if advertised_context_window.is_none() {
        if let Some(upstream_context_window) =
            template.get("context_window").and_then(context_window)
        {
            catalog_entry.insert("context_window".into(), upstream_context_window.into());
            if let Some(max_context_window) =
                template.get("max_context_window").and_then(context_window)
            {
                catalog_entry.insert("max_context_window".into(), max_context_window.into());
            }
        }
    }

    let mut catalog_value = Value::Object(catalog_entry);
    set_codex_service_tiers(
        &mut catalog_value,
        super::super::model_service_tiers(model, None),
    );
    codex_catalog_entry_is_compatible(&catalog_value).then_some(catalog_value)
}
