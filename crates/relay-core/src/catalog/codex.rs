use crate::DefaultServiceTier;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use serde_json::{json, Value};

mod shape;
pub use shape::{
    codex_catalog_entry_is_compatible, normalize_native_codex_catalog_entry,
    source_row_declares_reasoning,
};
use shape::{display_word, valid_model_id};

pub const CODEX_RELAY_ALIAS_PREFIX: &str = "zenith/";
pub const CODEX_RELAY_CATALOG_HASH: &str = "zenith-relay";
pub const CODEX_CATALOG_PRIORITY_BASE: u64 = 1_000;
const CODEX_RELAY_FALLBACK_CONTEXT_WINDOW: u64 = 272_000;

mod entry;

pub use entry::{
    apply_codex_ultra_from_official_model, normalize_codex_catalog_priorities,
    normalize_upstream_codex_catalog_entry, routed_codex_catalog_entry,
};

/// Publish the context window Codex needs before it will start auto-compact.
/// Native account cards are left untouched: Codex already knows those models.
/// A missing reference limit uses the same Relay fallback as routed rows, not
/// a theoretical million-token catalog value.
pub fn publish_routed_codex_context(entry: &mut Value, context_limit: Option<u64>) {
    let Some(object) = entry.as_object_mut() else {
        return;
    };
    let window = context_limit
        .filter(|window| *window > 0)
        .unwrap_or(CODEX_RELAY_FALLBACK_CONTEXT_WINDOW);
    let auto_compact = (window.saturating_mul(9) / 10).max(1);
    object.insert("context_window".into(), window.into());
    object.insert("max_context_window".into(), window.into());
    object.insert("auto_compact_token_limit".into(), auto_compact.into());
    object.insert("effective_context_window_percent".into(), 95.into());
}

/// Replace source-provided tier fields with the shared Relay model policy.
pub(crate) fn set_codex_service_tiers(model: &mut Value, supported: &[DefaultServiceTier]) {
    let Some(object) = model.as_object_mut() else {
        return;
    };
    let mut tiers = Vec::new();
    let mut aliases = Vec::new();
    for (tier, id, alias, name, description) in [
        (
            DefaultServiceTier::Fast,
            "priority",
            "fast",
            "Fast",
            "Priority processing for faster responses.",
        ),
        (
            DefaultServiceTier::Ultrafast,
            "ultrafast",
            "ultrafast",
            "Ultrafast",
            "Ultrafast processing for latency-sensitive work.",
        ),
    ] {
        if !supported.contains(&tier) {
            continue;
        }
        // Codex displays the catalog description for Ultrafast verbatim;
        // an empty string suppresses its built-in fallback text.
        tiers.push(json!({"id": id, "name": name, "description": description}));
        aliases.push(alias);
    }
    object.insert("service_tiers".into(), json!(tiers));
    // Older Codex clients consume this field instead of service_tiers.
    object.insert("additional_speed_tiers".into(), json!(aliases));
    object.remove("default_service_tier");
    object.remove("service_tier");
}

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
    codex_model_is_picker_eligible_for(model, true)
}

/// Picker eligibility with the operator policy for internal downgrade ids.
/// Media and transport exclusions stay in place when that policy is off.
pub fn codex_model_is_picker_eligible_for(model: &str, block_degraded_routes: bool) -> bool {
    if !valid_model_id(model) {
        return false;
    }
    if block_degraded_routes && super::order::is_degraded_route_model(model) {
        return false;
    }
    let id = crate::model_id_key(model);
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
    let leaf = super::order::model_leaf(model);
    // Use compact picker labels without the GPT prefix for numbered models.
    // Reference names take precedence at projection; routing IDs are unchanged.
    let leaf = leaf
        .get(..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("gpt-"))
        .and_then(|_| leaf.get(4..))
        .filter(|suffix| suffix.as_bytes().first().is_some_and(u8::is_ascii_digit))
        .unwrap_or(leaf);
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

#[cfg(test)]
mod tests;
