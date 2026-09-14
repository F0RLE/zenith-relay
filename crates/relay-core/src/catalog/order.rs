use std::collections::{BTreeMap, HashSet};

const MAX_MODEL_ID_BYTES: usize = 256;

#[derive(Clone, Copy)]
enum KnownModelFamily {
    OpenAi,
    Anthropic,
    Gemini,
    Grok,
    Zai,
}

/// Checks the common persisted model-ID boundary after callers trim their input.
pub fn is_valid_model_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_MODEL_ID_BYTES && !value.chars().any(char::is_control)
}

/// Checks a model ID that must be safe to use as one unescaped protocol token.
pub fn is_valid_model_token(value: &str) -> bool {
    is_valid_model_id(value) && !value.chars().any(char::is_whitespace)
}

/// Returns the persisted reasoning-policy key for a model. Known vendor
/// families share one operator choice; unknown models retain a per-model
/// setting because there is no safe grouping evidence.
pub fn reasoning_policy_key(model: &str) -> String {
    let leaf = model_leaf(model).to_ascii_lowercase();
    match known_model_family(&leaf) {
        Some(KnownModelFamily::OpenAi) => "group:openai".to_string(),
        Some(KnownModelFamily::Anthropic) => "group:anthropic".to_string(),
        Some(KnownModelFamily::Gemini) => "group:gemini".to_string(),
        Some(KnownModelFamily::Grok) => "group:grok".to_string(),
        Some(KnownModelFamily::Zai) => "group:zai".to_string(),
        None => model.trim().to_ascii_lowercase(),
    }
}

/// Looks up the shared policy first, then preserves a saved model-specific
/// setting from earlier Relay versions until the user edits that model.
///
/// A present empty vector is an explicit "disable every reported mode"
/// override; `None` means no override and therefore allows provider defaults.
pub fn reasoning_policy_levels<'a>(
    policies: &'a BTreeMap<String, Vec<String>>,
    model: &str,
) -> Option<&'a [String]> {
    let key = reasoning_policy_key(model);
    policies
        .get(&key)
        .or_else(|| policies.get(&model.trim().to_ascii_lowercase()))
        .map(Vec::as_slice)
}

/// Normalizes effort identifiers and keeps every level in the order used by
/// Codex and the Relay picker. Provider-specific/unknown identifiers remain
/// available after the known levels, preserving their first-seen order.
pub fn canonicalize_reasoning_levels<I, S>(levels: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for (source_order, level) in levels.into_iter().enumerate() {
        let level = level.as_ref().trim().to_ascii_lowercase();
        if !level.is_empty() && seen.insert(level.clone()) {
            ordered.push((source_order, level));
        }
    }
    ordered.sort_by(|(left_order, left), (right_order, right)| {
        reasoning_level_rank(left)
            .cmp(&reasoning_level_rank(right))
            .then_with(|| left_order.cmp(right_order))
    });
    ordered.into_iter().map(|(_, level)| level).collect()
}

fn reasoning_level_rank(level: &str) -> u8 {
    match level.replace('-', "_").as_str() {
        "none" => 0,
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        "xhigh" | "very_high" | "extra_high" => 5,
        "max" => 6,
        "ultra" => 7,
        _ => 8,
    }
}

/// Normalize and deduplicate model IDs while retaining discovery order.
/// Presentation ordering is supplied by the optional models.dev catalog.
pub fn canonicalize_model_ids<I, S>(models: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    normalize_model_ids(models)
}

/// Applies an operator's saved order while retaining discovery order for new
/// models. Catalog-aware callers use `ModelMetadataCatalog::merge_display_order`.
pub fn merge_model_display_order<I, S>(models: I, saved_order: &[String]) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let source_order = normalize_model_ids(models);
    let source_keys = source_order
        .iter()
        .map(|model| model.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    if !saved_order.iter().any(|model| {
        let key = model.trim().to_ascii_lowercase();
        !key.is_empty() && source_keys.contains(&key)
    }) {
        return source_order;
    }
    let catalog = source_order;
    let catalog_positions = catalog
        .iter()
        .enumerate()
        .map(|(position, model)| (model.to_ascii_lowercase(), position))
        .collect::<BTreeMap<_, _>>();
    let mut saved = HashSet::new();
    let mut ordered = Vec::with_capacity(catalog.len());
    for model in saved_order {
        let key = model.trim().to_ascii_lowercase();
        if !key.is_empty() && saved.insert(key.clone()) && catalog_positions.contains_key(&key) {
            ordered.push(catalog[catalog_positions[&key]].clone());
        }
    }
    for model in catalog {
        let key = model.to_ascii_lowercase();
        if saved.contains(&key) {
            continue;
        }
        let model_position = catalog_positions[&key];
        let insert_at = ordered.iter().position(|existing| {
            catalog_positions[&existing.to_ascii_lowercase()] > model_position
        });
        if let Some(insert_at) = insert_at {
            ordered.insert(insert_at, model);
        } else {
            ordered.push(model);
        }
    }
    ordered
}

/// Trim and de-duplicate model IDs while preserving the first spelling and
/// source order. This is the storage-normalization step shared by source
/// bindings and the runtime registry; it deliberately does not apply picker
/// grouping.
pub fn normalize_model_ids<I, S>(models: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    models
        .into_iter()
        .map(|model| model.as_ref().trim().to_string())
        .filter(|model| !model.is_empty())
        .filter(|model| seen.insert(model.to_ascii_lowercase()))
        .collect()
}

fn known_model_family(model: &str) -> Option<KnownModelFamily> {
    if is_openai_model(model) {
        Some(KnownModelFamily::OpenAi)
    } else if model.starts_with("claude-") {
        Some(KnownModelFamily::Anthropic)
    } else if model.starts_with("gemini-") {
        Some(KnownModelFamily::Gemini)
    } else if model.starts_with("grok-") {
        Some(KnownModelFamily::Grok)
    } else if model.starts_with("glm-") {
        Some(KnownModelFamily::Zai)
    } else {
        None
    }
}

fn model_leaf(model: &str) -> &str {
    model.rsplit('/').next().unwrap_or(model).trim()
}

fn is_openai_model(model: &str) -> bool {
    let is_reasoning =
        model.starts_with('o') && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit);
    model.starts_with("gpt-")
        || model.starts_with("codex-")
        || is_reasoning
        || model.starts_with("text-")
        || model.starts_with("dall-e")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_id_normalization_preserves_first_spelling_and_source_order() {
        assert_eq!(
            normalize_model_ids([
                " GPT-5 ".to_string(),
                "gpt-5".to_string(),
                "claude-sonnet".to_string(),
                "".to_string(),
                "CLAUDE-SONNET".to_string(),
            ]),
            ["GPT-5", "claude-sonnet"]
        );
    }

    #[test]
    fn saved_order_keeps_manual_anchors_and_inserts_new_models_by_discovery_order() {
        let ordered = merge_model_display_order(
            ["gpt-5.4", "gpt-6-astra", "gpt-5.5", "gpt-5.4-mini"],
            &["gpt-5.5".into(), "gpt-5.4".into()],
        );
        assert_eq!(
            ordered,
            ["gpt-6-astra", "gpt-5.5", "gpt-5.4", "gpt-5.4-mini"]
        );
        assert_eq!(
            ordered
                .iter()
                .filter(|model| ["gpt-5.5", "gpt-5.4"].contains(&model.as_str()))
                .map(String::as_str)
                .collect::<Vec<_>>(),
            ["gpt-5.5", "gpt-5.4"]
        );
    }

    #[test]
    fn model_id_validation_rejects_empty_control_and_oversized_values() {
        assert!(is_valid_model_id("gpt-test"));
        assert!(!is_valid_model_id(""));
        assert!(!is_valid_model_id("gpt\ntest"));
        assert!(!is_valid_model_id(&"x".repeat(MAX_MODEL_ID_BYTES + 1)));
        assert!(is_valid_model_token("gpt-test"));
        assert!(!is_valid_model_token("gpt test"));
    }

    #[test]
    fn reasoning_levels_use_the_codex_picker_order_and_keep_unknown_levels() {
        assert_eq!(
            canonicalize_reasoning_levels([
                "high",
                "very_high",
                "max",
                "low",
                "medium",
                "provider_custom",
                "low",
            ]),
            [
                "low",
                "medium",
                "high",
                "very_high",
                "max",
                "provider_custom",
            ]
        );
    }

    #[test]
    fn reasoning_policy_uses_the_openai_company_group() {
        let policies = BTreeMap::from([
            ("group:openai".to_string(), vec!["high".to_string()]),
            ("vendor/private-a".to_string(), vec!["low".to_string()]),
        ]);

        assert_eq!(reasoning_policy_key("vendor/gpt-5.6"), "group:openai");
        assert_eq!(
            reasoning_policy_levels(&policies, "vendor/gpt-5.7").map(ToOwned::to_owned),
            Some(vec!["high".to_string()])
        );
        let gpt_policy = BTreeMap::from([("group:openai".to_string(), vec!["max".to_string()])]);
        assert_eq!(
            reasoning_policy_levels(&gpt_policy, "gpt-5.7").map(ToOwned::to_owned),
            Some(vec!["max".to_string()])
        );
        assert_eq!(reasoning_policy_key("o3"), "group:openai");
        assert_eq!(reasoning_policy_key("vendor/private-a"), "vendor/private-a");
        assert_eq!(
            reasoning_policy_levels(&policies, "vendor/private-a").map(ToOwned::to_owned),
            Some(vec!["low".to_string()])
        );
        assert_eq!(reasoning_policy_levels(&policies, "vendor/private-b"), None);
    }
}
