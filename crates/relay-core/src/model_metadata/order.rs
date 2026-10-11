//! Presentation order for catalog models.
//!
//! The metadata catalog supplies the provider identity used for grouping. It
//! must not invent a model ranking from release dates, family names, or model
//! IDs: the provider/account/source sequence is the only upstream ordering
//! evidence Relay has. Manual model order remains a separate override.

use chrono::{Datelike, NaiveDate};
use std::cmp::Ordering;

use super::ModelMetadataCatalog;

/// Compare provider blocks while leaving models from one provider equal.
///
/// `ModelMetadataCatalog::order_model_ids` applies this comparator with the
/// original source position as the final key. That gives us stable provider
/// blocks without reordering the models returned by an account or API source.
pub(super) fn compare_model_groups(
    catalog: &ModelMetadataCatalog,
    left_id: &str,
    right_id: &str,
) -> Ordering {
    let left_provider = catalog.display_provider_for(left_id);
    let right_provider = catalog.display_provider_for(right_id);

    match (left_provider, right_provider) {
        (Some(left), Some(right)) => provider_order(&left).cmp(&provider_order(&right)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

pub(super) fn canonical_namespace(model_id: &str) -> Option<String> {
    let (namespace, model) = model_id.trim().split_once('/')?;
    if model.is_empty() {
        return None;
    }
    provider_key(namespace)
}

/// Keep common provider aliases in one presentation block. The metadata
/// itself stays untouched so it remains an accurate source identity.
fn canonical_provider(provider: &str) -> String {
    match normalize(provider).as_str() {
        "x-ai" | "x_ai" => "xai".to_string(),
        normalized => normalized.to_string(),
    }
}

/// Known provider blocks have a deterministic leading order. Other companies
/// follow alphabetically; models inside every block retain source order.
fn provider_order(provider: &str) -> (u8, &str) {
    let rank = match provider {
        "openai" => 0,
        "anthropic" => 1,
        "google" => 2,
        "xai" => 3,
        _ => 4,
    };
    (rank, provider)
}

/// Display-only provider for a native model ID when the reference catalog has
/// no exact or unique leaf match. This keeps discovered ChatGPT models in the
/// OpenAI block without changing their route IDs.
pub(super) fn inferred_provider(model_id: &str) -> Option<String> {
    let normalized = normalize(model_id);
    let model_leaf_id = model_leaf(strip_reasoning_effort(&normalized));
    let provider = if is_openai_model(model_leaf_id) {
        "openai"
    } else if model_leaf_id.starts_with("claude-") {
        "anthropic"
    } else if model_leaf_id.starts_with("gemini-")
        || model_leaf_id.starts_with("gemma-")
        || model_leaf_id.starts_with("imagen-")
        || model_leaf_id.starts_with("veo-")
    {
        "google"
    } else if model_leaf_id.starts_with("grok-") {
        "xai"
    } else {
        return None;
    };
    Some(provider.to_string())
}

fn is_openai_model(model: &str) -> bool {
    let is_reasoning =
        model.starts_with('o') && model.as_bytes().get(1).is_some_and(u8::is_ascii_digit);
    is_reasoning
        || model.starts_with("gpt-")
        || model.starts_with("chatgpt-")
        || model.starts_with("codex-")
        || model.starts_with("dall-e")
        || model.starts_with("text-")
        || model.starts_with("tts-")
        || model.starts_with("whisper-")
        || model.starts_with("sora-")
        || model.starts_with("computer-use-")
}

pub(super) fn provider_key(provider: &str) -> Option<String> {
    let provider = canonical_provider(provider);
    (!provider.is_empty()).then_some(provider)
}

fn strip_reasoning_effort(model: &str) -> &str {
    const SUFFIXES: &[&str] = &[
        "-non-reasoning",
        "-xhigh",
        "-minimal",
        "-medium",
        "-high",
        "-low",
        "-max",
        "-none",
    ];
    for suffix in SUFFIXES {
        if let Some(stripped) = model.strip_suffix(suffix) {
            if !stripped.is_empty() && !stripped.ends_with('-') {
                return stripped;
            }
        }
    }
    model
}

pub(super) fn normalize(provider_or_model: &str) -> String {
    provider_or_model.trim().to_ascii_lowercase()
}

pub(super) fn model_leaf(model_id: &str) -> &str {
    model_id.rsplit('/').next().unwrap_or(model_id)
}

pub(super) fn date_key(date_text: &str) -> Option<u32> {
    let date = NaiveDate::parse_from_str(date_text, "%Y-%m-%d")
        .ok()
        .or_else(|| {
            let (year, month) = date_text.split_once('-')?;
            if month.len() != 2 {
                return None;
            }
            NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)
        })?;
    let year = u32::try_from(date.year()).ok()?;
    Some(year.saturating_mul(10_000) + date.month() * 100 + date.day())
}
