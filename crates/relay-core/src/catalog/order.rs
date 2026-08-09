use std::collections::HashSet;

const MAX_MODEL_ID_BYTES: usize = 256;

/// The launcher-facing order is based only on a model ID. It deliberately has
/// no source/provider identity: sources merely publish model IDs, while the
/// picker groups familiar model families and keeps every unknown ID in the
/// response order in which it first appeared.
const OPENAI_MODEL_ORDER: &[&str] = &[
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-5.6-luna",
    "gpt-5.5",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-image-2",
];

const CLAUDE_MODEL_ORDER: &[&str] = &[
    "claude-fable-5",
    "claude-opus-5",
    "claude-opus-4-8",
    "claude-opus-4-7",
    "claude-opus-4-6",
    "claude-sonnet-5",
    "claude-sonnet-4-6",
    "claude-haiku-4-5",
];

const GEMINI_MODEL_ORDER: &[&str] = &[
    "gemini-3.1-pro-preview",
    "gemini-3.6-flash-high",
    "gemini-3.6-flash-medium",
    "gemini-3.6-flash-low",
];

const GLM_MODEL_ORDER: &[&str] = &["glm-5.2", "glm-5.1", "glm-5-turbo", "glm-4.7"];

#[derive(Clone, Debug)]
struct OrderedModel {
    id: String,
    upstream_order: usize,
}

/// Checks the common persisted model-ID boundary after callers trim their input.
pub fn is_valid_model_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_MODEL_ID_BYTES && !value.chars().any(char::is_control)
}

/// Checks a model ID that must be safe to use as one unescaped protocol token.
pub fn is_valid_model_token(value: &str) -> bool {
    is_valid_model_id(value) && !value.chars().any(char::is_whitespace)
}

/// Normalize, deduplicate, and order model IDs for a launcher or Codex picker.
///
/// This function is intentionally not used by Relay's public runtime catalog:
/// that catalog preserves the source response order. Known model families are
/// grouped here for presentation only. Unknown models never get an
/// alphabetical tie-breaker.
pub fn canonicalize_model_ids<I, S>(models: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut seen = HashSet::new();
    let mut ordered = Vec::new();
    for (upstream_order, model) in models.into_iter().enumerate() {
        let model = model.as_ref().trim();
        if model.is_empty() {
            continue;
        }
        let normalized = model.to_ascii_lowercase();
        if seen.insert(normalized) {
            ordered.push(OrderedModel {
                id: model.to_string(),
                upstream_order,
            });
        }
    }
    ordered.sort_by_key(|model| {
        (
            model_group_rank(&model.id),
            model_rank(&model.id).unwrap_or(u16::MAX),
            model.upstream_order,
        )
    });
    ordered.into_iter().map(|model| model.id).collect()
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

fn model_group_rank(model: &str) -> u8 {
    let model = model_leaf(model).to_ascii_lowercase();
    if is_openai_model(&model) {
        0
    } else if model.starts_with("claude-") {
        1
    } else if model.starts_with("gemini-") {
        2
    } else if model.starts_with("glm-") {
        3
    } else if model.starts_with("grok-") {
        4
    } else {
        5
    }
}

fn model_rank(model: &str) -> Option<u16> {
    let model = model_leaf(model).to_ascii_lowercase();
    [
        OPENAI_MODEL_ORDER,
        CLAUDE_MODEL_ORDER,
        GEMINI_MODEL_ORDER,
        GLM_MODEL_ORDER,
    ]
    .into_iter()
    .flat_map(|models| models.iter().copied().enumerate())
    .find_map(|(rank, known)| (known == model).then_some(rank as u16))
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
    fn known_families_follow_the_launcher_contract() {
        let models = canonicalize_model_ids([
            "private-second",
            "glm-4.7",
            "grok-new",
            "gemini-3.6-flash-low",
            "claude-haiku-4-5",
            "gpt-5.4-mini",
            "gpt-5.6-sol",
            "glm-5.2",
            "private-first",
            "claude-fable-5",
            "gemini-3.1-pro-preview",
            "gpt-image-2",
            "gpt-5.5",
            "GPT-5.6-SOL",
        ]);

        assert_eq!(
            models,
            [
                "gpt-5.6-sol",
                "gpt-5.5",
                "gpt-5.4-mini",
                "gpt-image-2",
                "claude-fable-5",
                "claude-haiku-4-5",
                "gemini-3.1-pro-preview",
                "gemini-3.6-flash-low",
                "glm-5.2",
                "glm-4.7",
                "grok-new",
                "private-second",
                "private-first",
            ]
        );
    }

    #[test]
    fn namespaced_models_are_grouped_by_their_model_id_not_source() {
        assert_eq!(
            canonicalize_model_ids([
                "source-b/claude-opus-4-8",
                "source-a/gpt-5.4",
                "source-c/unknown",
            ]),
            [
                "source-a/gpt-5.4",
                "source-b/claude-opus-4-8",
                "source-c/unknown",
            ]
        );
    }

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
    fn model_id_validation_rejects_empty_control_and_oversized_values() {
        assert!(is_valid_model_id("gpt-test"));
        assert!(!is_valid_model_id(""));
        assert!(!is_valid_model_id("gpt\ntest"));
        assert!(!is_valid_model_id(&"x".repeat(MAX_MODEL_ID_BYTES + 1)));
        assert!(is_valid_model_token("gpt-test"));
        assert!(!is_valid_model_token("gpt test"));
    }
}
