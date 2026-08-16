use std::{cmp::Ordering, collections::HashSet};

const MAX_MODEL_ID_BYTES: usize = 256;

#[derive(Clone, Copy)]
enum KnownModelFamily {
    OpenAi,
    Anthropic,
    Gemini,
    Grok,
    Zai,
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct SemanticModelSortKey {
    family_rank: u8,
    image_rank: u8,
    tier_rank: u8,
    version_rank: Vec<i64>,
    modifier_rank: u8,
    preview_rank: u8,
    model: String,
}

#[derive(Clone, Debug)]
struct OrderedModel {
    id: String,
    upstream_order: usize,
    semantic_key: Option<SemanticModelSortKey>,
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
/// Familiar model families use a semantic hierarchy: company, model class,
/// version, and release modifier. Unknown IDs retain their upstream order.
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
                semantic_key: semantic_model_sort_key(model),
            });
        }
    }
    ordered.sort_by(
        |left, right| match (&left.semantic_key, &right.semantic_key) {
            (Some(left_key), Some(right_key)) => left_key
                .cmp(right_key)
                .then_with(|| left.upstream_order.cmp(&right.upstream_order)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => left.upstream_order.cmp(&right.upstream_order),
        },
    );
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

fn semantic_model_sort_key(model: &str) -> Option<SemanticModelSortKey> {
    let model = model_leaf(model).to_ascii_lowercase();
    let family = known_model_family(&model)?;
    let is_image = model_has_term(&model, "image") || model.starts_with("dall-e");
    Some(SemanticModelSortKey {
        family_rank: model_family_rank(family),
        image_rank: u8::from(is_image),
        tier_rank: model_tier_rank(family, &model, is_image),
        version_rank: model_version_rank(family, &model),
        modifier_rank: model_modifier_rank(family, &model),
        preview_rank: u8::from(model_has_term(&model, "preview")),
        model,
    })
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

fn model_family_rank(family: KnownModelFamily) -> u8 {
    match family {
        KnownModelFamily::OpenAi => 0,
        KnownModelFamily::Anthropic => 1,
        KnownModelFamily::Gemini => 2,
        KnownModelFamily::Grok => 3,
        KnownModelFamily::Zai => 4,
    }
}

fn model_tier_rank(family: KnownModelFamily, model: &str, is_image: bool) -> u8 {
    match family {
        KnownModelFamily::Anthropic if model_has_term(model, "fable") => 0,
        KnownModelFamily::Anthropic if model_has_term(model, "opus") => 1,
        KnownModelFamily::Anthropic if model_has_term(model, "sonnet") => 2,
        KnownModelFamily::Anthropic if model_has_term(model, "haiku") => 3,
        KnownModelFamily::Gemini | KnownModelFamily::OpenAi if is_image => 90,
        KnownModelFamily::Gemini if model_has_term(model, "pro") => 0,
        KnownModelFamily::Gemini if model_has_term(model, "lite") => 2,
        KnownModelFamily::Gemini if model_has_term(model, "flash") => 1,
        KnownModelFamily::OpenAi
            if model_has_term(model, "mini") || model_has_term(model, "compact") =>
        {
            10
        }
        KnownModelFamily::OpenAi if model_has_term(model, "spark") => 20,
        KnownModelFamily::Grok if model_has_term(model, "build") => 10,
        KnownModelFamily::Zai
            if model_has_term(model, "air")
                || model_has_term(model, "flash")
                || model_has_term(model, "lite") =>
        {
            10
        }
        KnownModelFamily::Anthropic | KnownModelFamily::Gemini => 80,
        _ if is_image => 9,
        _ => 0,
    }
}

fn model_modifier_rank(family: KnownModelFamily, model: &str) -> u8 {
    match family {
        KnownModelFamily::OpenAi if model.ends_with("-sol") => 1,
        KnownModelFamily::OpenAi if model.ends_with("-terra") => 2,
        KnownModelFamily::OpenAi if model.ends_with("-luna") => 3,
        KnownModelFamily::OpenAi => 8,
        KnownModelFamily::Gemini if model_has_term(model, "preview") => 9,
        KnownModelFamily::Gemini if model.ends_with("-high") => 1,
        KnownModelFamily::Gemini if model.ends_with("-medium") => 2,
        KnownModelFamily::Gemini if model.ends_with("-low") => 3,
        KnownModelFamily::Grok if model.ends_with("-non-reasoning") => 1,
        KnownModelFamily::Grok if model.ends_with("-reasoning") => 0,
        _ => 0,
    }
}

fn model_version_rank(family: KnownModelFamily, model: &str) -> Vec<i64> {
    let mut version = model_version_components(family, model);
    if matches!(family, KnownModelFamily::Grok) && is_dated_grok_release(model) {
        if let Some(minor) = version.get_mut(1) {
            if *minor >= 10 && *minor % 10 == 0 {
                *minor /= 10;
            }
        }
    }
    if version.is_empty() {
        vec![0]
    } else {
        version.resize(4, 0);
        version.into_iter().map(|part| -part).collect()
    }
}

fn model_version_components(family: KnownModelFamily, model: &str) -> Vec<i64> {
    let tokens = model.split('-').collect::<Vec<_>>();
    let Some(first_version_token) = tokens
        .iter()
        .position(|token| token.bytes().any(|byte| byte.is_ascii_digit()))
    else {
        return Vec::new();
    };

    if !matches!(family, KnownModelFamily::Anthropic) {
        return version_token_components(tokens[first_version_token]);
    }

    let mut version = Vec::with_capacity(4);
    for token in &tokens[first_version_token..] {
        if token.len() > 5 && token.bytes().all(|byte| byte.is_ascii_digit()) {
            break;
        }
        let components = version_token_components(token);
        if components.is_empty() {
            break;
        }
        version.extend(components);
        if version.len() >= 4 {
            break;
        }
    }
    version.truncate(4);
    version
}

fn version_token_components(token: &str) -> Vec<i64> {
    token
        .split(|ch: char| !ch.is_ascii_digit())
        .filter(|part| !part.is_empty() && part.len() <= 5)
        .filter_map(|part| part.parse::<i64>().ok())
        .take(4)
        .collect()
}

fn is_dated_grok_release(model: &str) -> bool {
    let mut parts = model.split('-');
    let (Some("grok"), Some(version), Some(release)) = (parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    version
        .split('.')
        .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        && release.len() == 4
        && release.bytes().all(|byte| byte.is_ascii_digit())
}

fn model_has_term(model: &str, term: &str) -> bool {
    model
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .any(|part| part == term)
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
    fn semantic_catalog_order_matches_the_public_model_hierarchy() {
        let models = canonicalize_model_ids([
            "private-second",
            "grok-build-0.1",
            "grok-4.20-0309-non-reasoning",
            "grok-4.20-0309-reasoning",
            "grok-4.3",
            "grok-4.5",
            "grok-4.6",
            "gemini-2.5-flash-lite",
            "gemini-3.1-flash-lite",
            "gemini-2.5-flash",
            "gemini-3-flash-preview",
            "gemini-3-flash",
            "gemini-3.5-flash",
            "gemini-3.6-flash",
            "gemini-3.7-flash",
            "gemini-2.5-pro",
            "gemini-3-pro-preview",
            "gemini-3-pro",
            "gemini-3.1-pro-preview",
            "claude-haiku-4-5",
            "claude-sonnet-4-6",
            "claude-sonnet-5",
            "claude-opus-4-6",
            "claude-opus-4-7",
            "claude-opus-4-8",
            "claude-opus-5",
            "claude-fable-5",
            "gpt-5.4-mini",
            "gpt-5.4",
            "gpt-5.5",
            "gpt-5.6-terra",
            "gpt-5.6-sol",
            "private-first",
        ]);

        assert_eq!(
            models,
            [
                "gpt-5.6-sol",
                "gpt-5.6-terra",
                "gpt-5.5",
                "gpt-5.4",
                "gpt-5.4-mini",
                "claude-fable-5",
                "claude-opus-5",
                "claude-opus-4-8",
                "claude-opus-4-7",
                "claude-opus-4-6",
                "claude-sonnet-5",
                "claude-sonnet-4-6",
                "claude-haiku-4-5",
                "gemini-3.1-pro-preview",
                "gemini-3-pro",
                "gemini-3-pro-preview",
                "gemini-2.5-pro",
                "gemini-3.7-flash",
                "gemini-3.6-flash",
                "gemini-3.5-flash",
                "gemini-3-flash",
                "gemini-3-flash-preview",
                "gemini-2.5-flash",
                "gemini-3.1-flash-lite",
                "gemini-2.5-flash-lite",
                "grok-4.6",
                "grok-4.5",
                "grok-4.3",
                "grok-4.20-0309-reasoning",
                "grok-4.20-0309-non-reasoning",
                "grok-build-0.1",
                "private-second",
                "private-first",
            ]
        );
    }

    #[test]
    fn future_models_use_family_tier_version_and_modifier_not_a_static_catalog() {
        assert_eq!(
            canonicalize_model_ids([
                "private-first",
                "grok-5.20-0612-non-reasoning",
                "grok-5.20-0612-reasoning",
                "grok-5.6",
                "grok-5.6.1",
                "gemini-4.1-flash",
                "gemini-4.1.2-flash",
                "gemini-3.9-pro",
                "gemini-4-pro",
                "claude-sonnet-6",
                "claude-opus-5",
                "claude-opus-6",
                "claude-opus-6-1",
                "gpt-6.2-terra",
                "gpt-6.2.1-terra",
                "gpt-6.2-sol",
                "gpt-6.2-experimental",
                "gpt-7.0",
                "private-second",
            ]),
            [
                "gpt-7.0",
                "gpt-6.2.1-terra",
                "gpt-6.2-sol",
                "gpt-6.2-terra",
                "gpt-6.2-experimental",
                "claude-opus-6-1",
                "claude-opus-6",
                "claude-opus-5",
                "claude-sonnet-6",
                "gemini-4-pro",
                "gemini-3.9-pro",
                "gemini-4.1.2-flash",
                "gemini-4.1-flash",
                "grok-5.6.1",
                "grok-5.6",
                "grok-5.20-0612-reasoning",
                "grok-5.20-0612-non-reasoning",
                "private-first",
                "private-second",
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
