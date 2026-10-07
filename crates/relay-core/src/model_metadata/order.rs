//! Presentation order for catalog models.
//!
//! The metadata catalog supplies the provider identity used for grouping. It
//! must not invent a model ranking from release dates, family names, or model
//! IDs: the provider/account/source sequence is the only upstream ordering
//! evidence Relay has. Manual model order remains a separate override.

use chrono::{Datelike, NaiveDate};
use std::cmp::Ordering;

use super::ModelMetadata;

/// Compare provider blocks while leaving models from one provider equal.
///
/// `ModelMetadataCatalog::order_model_ids` applies this comparator with the
/// original source position as the final key. That gives us stable provider
/// blocks without reordering the models returned by an account or API source.
pub(super) fn compare_metadata(
    left_id: &str,
    left: Option<&ModelMetadata>,
    right_id: &str,
    right: Option<&ModelMetadata>,
) -> Ordering {
    let left_provider = presentation_provider(left_id, left);
    let right_provider = presentation_provider(right_id, right);

    match (left_provider, right_provider) {
        (Some(left), Some(right)) => {
            let left = canonical_provider(&left);
            let right = canonical_provider(&right);
            provider_order(&left)
                .cmp(&provider_order(&right))
                .then_with(|| left.cmp(&right))
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

fn presentation_provider(id: &str, metadata: Option<&ModelMetadata>) -> Option<String> {
    metadata
        .and_then(provider_key)
        .or_else(|| inferred_provider(id))
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
fn inferred_provider(id: &str) -> Option<String> {
    let normalized = normalize(id);
    let model = model_leaf(strip_reasoning_effort(&normalized));
    let provider = if is_openai_model(model) {
        "openai"
    } else if model.starts_with("claude-") {
        "anthropic"
    } else if model.starts_with("gemini-")
        || model.starts_with("gemma-")
        || model.starts_with("imagen-")
        || model.starts_with("veo-")
    {
        "google"
    } else if model.starts_with("grok-") {
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

fn provider_key(metadata: &ModelMetadata) -> Option<String> {
    let provider = canonical_provider(&metadata.provider);
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

pub(super) fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

pub(super) fn model_leaf(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

pub(super) fn date_key(value: &str) -> Option<u32> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .or_else(|| {
            let (year, month) = value.split_once('-')?;
            if month.len() != 2 {
                return None;
            }
            NaiveDate::from_ymd_opt(year.parse().ok()?, month.parse().ok()?, 1)
        })?;
    let year = u32::try_from(date.year()).ok()?;
    Some(year.saturating_mul(10_000) + date.month() * 100 + date.day())
}
