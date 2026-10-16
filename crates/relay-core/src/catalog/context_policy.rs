use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

use super::is_valid_model_id;

/// Internal auto-compaction: Relay sets this share of the selected window
/// as the downstream client compaction threshold. Not user-configurable.
const AUTO_COMPACTION_PERCENT: u64 = 90;

/// Fallback only when the model has no published default window.
pub const SHORT_CONTEXT_PERCENT: u64 = 25;

/// Original window evidence, never the operator's already-projected budget.
/// An empty value means unknown, not permission to borrow an API maximum
/// for a native account that did not publish its own card.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelContextWindows {
    pub default_tokens: Option<u64>,
    pub maximum_tokens: Option<u64>,
}

impl ModelContextWindows {
    pub(crate) fn from_entry(entry: &Value) -> Self {
        let positive = |field| {
            entry
                .get(field)
                .and_then(Value::as_u64)
                .filter(|tokens| *tokens > 0)
        };
        Self {
            default_tokens: positive("context_window"),
            maximum_tokens: positive("max_context_window"),
        }
    }

    pub fn window_tokens(self, selected: ModelContextPolicy) -> Option<u64> {
        context_window_tokens(selected, self.default_tokens, self.maximum_tokens)
    }
}

/// Select a client window without inventing an unpublished model maximum.
/// A published default is usable capacity, but does not cap an explicit custom window.
pub fn context_window_tokens(
    selected: ModelContextPolicy,
    default_window: Option<u64>,
    maximum_window: Option<u64>,
) -> Option<u64> {
    let maximum = maximum_window.filter(|tokens| *tokens > 0);
    let default = default_window.filter(|tokens| *tokens > 0);
    match selected {
        // A published default is the model's Short window. Never replace it
        // with a fraction of its Long window or a pricing-band threshold.
        ModelContextPolicy::Short => default
            .map(|tokens| maximum.map_or(tokens, |maximum| tokens.min(maximum)))
            .or_else(|| {
                maximum.map(|tokens| {
                    (tokens.saturating_mul(SHORT_CONTEXT_PERCENT) / 100)
                        .max(1_024)
                        .min(tokens)
                })
            }),
        ModelContextPolicy::Extended => maximum.or(default),
        ModelContextPolicy::Custom { tokens } => {
            Some(maximum.map_or(tokens, |maximum| tokens.min(maximum)))
        }
    }
}

/// Per-model context choice set in Pool, Model Rules.
///
/// Short uses the published default, or 25% of a known maximum only when
/// no default was published. Extended uses all known capacity. Custom lets the
/// user specify a token count directly and works for any model, even when
/// the provider did not publish a short/long pair.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ModelContextPolicy {
    /// The model's default window, with a capacity-derived fallback.
    #[default]
    Short,
    /// The provider's maximum window, or its published default when unknown.
    Extended,
    /// A user-specified token count. Always available.
    Custom { tokens: u64 },
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ModelContextPolicyInput {
    Legacy(ModelContextPolicyMode),
    Structured(ModelContextPolicyDocument),
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ModelContextPolicyMode {
    Short,
    Extended,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode", deny_unknown_fields)]
enum ModelContextPolicyDocument {
    Short,
    Extended,
    Custom { tokens: u64 },
}

impl<'de> Deserialize<'de> for ModelContextPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match ModelContextPolicyInput::deserialize(deserializer)? {
            ModelContextPolicyInput::Legacy(ModelContextPolicyMode::Short) => Ok(Self::Short),
            ModelContextPolicyInput::Legacy(ModelContextPolicyMode::Extended) => Ok(Self::Extended),
            ModelContextPolicyInput::Structured(ModelContextPolicyDocument::Short) => {
                Ok(Self::Short)
            }
            ModelContextPolicyInput::Structured(ModelContextPolicyDocument::Extended) => {
                Ok(Self::Extended)
            }
            ModelContextPolicyInput::Structured(ModelContextPolicyDocument::Custom { tokens }) => {
                Ok(Self::Custom { tokens })
            }
        }
    }
}

impl ModelContextPolicy {
    pub fn is_custom(self) -> bool {
        matches!(self, Self::Custom { .. })
    }

    pub fn validate(self) -> Result<(), &'static str> {
        if let Self::Custom { tokens } = self {
            if !(1_024..=2_000_000).contains(&tokens) {
                return Err("custom context must be between 1024 and 2000000 tokens");
            }
        }
        Ok(())
    }
}

/// Normalize saved per-model context choices.
pub fn normalize_model_context_policies(
    policies: BTreeMap<String, ModelContextPolicy>,
) -> Result<BTreeMap<String, ModelContextPolicy>, &'static str> {
    let mut normalized = BTreeMap::new();
    for (model_id, policy) in policies {
        let model_id = model_id.trim();
        if !is_valid_model_id(model_id) {
            return Err("model context policy has an invalid model id");
        }
        policy.validate()?;
        normalized.insert(crate::model_id_key(model_id), policy);
    }
    Ok(normalized)
}

/// A model has a provider-published long window.
pub fn entry_has_long_window(entry: &Value) -> bool {
    let window = |key: &str| {
        entry
            .get(key)
            .and_then(Value::as_u64)
            .filter(|tokens| *tokens > 0)
    };
    matches!(
        (window("context_window"), window("max_context_window")),
        (Some(current), Some(maximum)) if maximum > current
    )
}

/// Context windows apply only to models that produce text output.
///
/// Multimodal models that also return text remain eligible. Unknown models
/// stay eligible until the catalog identifies a non-text family or output.
pub fn model_supports_text_context(
    model_id: &str,
    family: Option<&str>,
    output_modalities: &[String],
) -> bool {
    let is_non_text_family = family
        .unwrap_or_default()
        .to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .any(|token| {
            [
                "audio",
                "dalle",
                "embed",
                "embedding",
                "image",
                "moderation",
                "speech",
                "tts",
                "video",
            ]
            .contains(&token)
        });
    if is_non_text_family {
        return false;
    }

    let model_id = model_id.trim().to_ascii_lowercase();
    if [
        "audio",
        "dall-e",
        "dalle",
        "embed",
        "embedding",
        "gpt-image",
        "moderation",
        "tts",
        "video",
        "whisper",
    ]
    .iter()
    .any(|token| model_id.starts_with(token) || model_id.contains(&format!("-{token}")))
    {
        return false;
    }

    let outputs = output_modalities
        .iter()
        .map(|modality| modality.trim().to_ascii_lowercase())
        .filter(|modality| !modality.is_empty())
        .collect::<Vec<_>>();
    let has_text_output = outputs
        .iter()
        .any(|modality| modality == "text" || modality.contains("text"));
    if !outputs.is_empty() && !has_text_output {
        return false;
    }
    true
}

/// Read modality metadata from a projected catalog entry before applying a
/// user-selected context policy.
fn entry_supports_text_context(entry: &Value) -> bool {
    let string_field = |key: &str| entry.get(key).and_then(Value::as_str);
    let model_id = string_field("id")
        .or_else(|| string_field("slug"))
        .or_else(|| string_field("model"))
        .unwrap_or_default();
    let family = string_field("family").or_else(|| string_field("catalog_family"));
    let output_modalities = entry
        .get("output_modalities")
        .or_else(|| {
            entry
                .get("modalities")
                .and_then(|modalities| modalities.get("output"))
        })
        .and_then(Value::as_array)
        .map(|modalities| {
            modalities
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    model_supports_text_context(model_id, family, &output_modalities)
}

/// Apply a per-model context policy to a catalog entry. Always called by the
/// runtime; there is no global on/off switch.
///
/// Preserves a published default for Short; only absent defaults use a ratio.
/// With no maximum, Short and Extended preserve a published default, if any.
/// Sets auto_compact_token_limit to 90 percent of the window, lowered by
/// hard model limits. Sets effective_context_window_percent to 100 when
/// the window differs from the provider default.
pub fn apply_model_context(
    entry: &mut Value,
    selected: ModelContextPolicy,
    context_limit: Option<u64>,
    input_limit: Option<u64>,
    output_limit: Option<u64>,
) {
    apply_context_policy(entry, selected, context_limit, input_limit, output_limit);
}

/// Native account cards own their default window independently of their
/// reference metadata. Resolve budgets from their own windows only.
pub(crate) fn apply_native_model_context(
    entry: &mut Value,
    selected: ModelContextPolicy,
    input_limit: Option<u64>,
    output_limit: Option<u64>,
) {
    apply_context_policy(entry, selected, None, input_limit, output_limit);
}

fn apply_context_policy(
    entry: &mut Value,
    selected: ModelContextPolicy,
    context_limit: Option<u64>,
    input_limit: Option<u64>,
    output_limit: Option<u64>,
) {
    if !entry_supports_text_context(entry) {
        return;
    }
    let positive = |value: Option<u64>| value.filter(|number| *number > 0);
    let default = positive(entry.get("context_window").and_then(Value::as_u64));
    let native_max = positive(entry.get("max_context_window").and_then(Value::as_u64));
    // A published default is not a hard maximum. Only clamp a custom window
    // when a route or model explicitly declares its actual capacity.
    let hard_maximum = [native_max, positive(context_limit)]
        .into_iter()
        .flatten()
        .min();

    let Some(window) = context_window_tokens(selected, default, hard_maximum) else {
        return;
    };
    let overrides_default = default != Some(window) || selected.is_custom();

    let auto_compact = (window.saturating_mul(AUTO_COMPACTION_PERCENT) / 100)
        .min(positive(input_limit).unwrap_or(u64::MAX))
        .min(
            hard_maximum
                .map(|maximum| maximum.saturating_sub(positive(output_limit).unwrap_or(0)))
                .unwrap_or(u64::MAX),
        )
        .max(1);

    if overrides_default {
        entry["context_window"] = json!(window);
        if let Some(maximum) = hard_maximum {
            entry["max_context_window"] = json!(maximum);
        }
        entry["effective_context_window_percent"] = json!(100);
    }
    entry["auto_compact_token_limit"] = json!(auto_compact);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_preserves_published_window_and_sets_auto_compact() {
        let mut entry = json!({"context_window": 200000, "max_context_window": 900000});
        apply_model_context(&mut entry, ModelContextPolicy::Short, None, None, None);
        assert_eq!(entry["context_window"], 200000);
        assert_eq!(entry["auto_compact_token_limit"], 180_000);
        let projected = entry.clone();
        apply_model_context(&mut entry, ModelContextPolicy::Short, None, None, None);
        assert_eq!(entry, projected);
    }

    #[test]
    fn native_and_api_budgets_obey_their_own_maximum() {
        let card = json!({
            "context_window": 128000,
            "max_context_window": 120000,
            "effective_context_window_percent": 90
        });
        let mut native = card.clone();
        apply_native_model_context(&mut native, ModelContextPolicy::Short, None, Some(16000));
        assert_eq!(native["context_window"], 120000);
        assert_eq!(native["max_context_window"], 120000);
        assert_eq!(native["effective_context_window_percent"], 100);
        assert_eq!(native["auto_compact_token_limit"], 104000);

        let mut api = card;
        apply_model_context(&mut api, ModelContextPolicy::Short, None, None, None);
        assert_eq!(api["context_window"], 120000);
        assert_eq!(api["effective_context_window_percent"], 100);
        assert_eq!(api["auto_compact_token_limit"], 108000);
    }

    #[test]
    fn known_maximum_supplies_a_selected_budget_without_a_provider_default() {
        for (selected, window) in [
            (ModelContextPolicy::Short, 16000),
            (ModelContextPolicy::Extended, 64000),
        ] {
            let mut entry = json!({"max_context_window": 64000});
            apply_model_context(&mut entry, selected, Some(64000), None, None);
            assert_eq!(entry["context_window"], window);
            assert_eq!(entry["effective_context_window_percent"], 100);
            assert_eq!(entry["max_context_window"], 64_000);
            assert_eq!(entry["auto_compact_token_limit"], window * 90 / 100);
        }
    }

    #[test]
    fn extended_uses_provider_maximum_when_available() {
        let card = json!({"context_window": 200000, "max_context_window": 900000});
        let mut short = card.clone();
        apply_model_context(&mut short, ModelContextPolicy::Short, None, None, None);
        assert_eq!(short["context_window"], 200000);
        let mut long = card.clone();
        apply_model_context(&mut long, ModelContextPolicy::Extended, None, None, None);
        assert_eq!(long["context_window"], 900000);
        assert_eq!(long["max_context_window"], 900000);
        assert_eq!(long["auto_compact_token_limit"], 810_000);
        assert_eq!(long["effective_context_window_percent"], 100);
    }

    #[test]
    fn hard_limits_lower_auto_compact() {
        let mut entry = json!({"context_window": 128000, "max_context_window": 256000});
        apply_model_context(
            &mut entry,
            ModelContextPolicy::Extended,
            Some(200000),
            Some(150000),
            Some(32000),
        );
        assert_eq!(entry["context_window"], 200000);
        assert_eq!(entry["auto_compact_token_limit"], 150_000);
    }

    #[test]
    fn custom_sets_user_window_even_without_provider_pair() {
        let mut entry = json!({"context_window": 128000});
        apply_model_context(
            &mut entry,
            ModelContextPolicy::Custom { tokens: 100000 },
            None,
            None,
            None,
        );
        assert_eq!(entry["context_window"], 100000);
        assert_eq!(entry["auto_compact_token_limit"], 90_000);
        assert_eq!(entry["effective_context_window_percent"], 100);
    }

    #[test]
    fn custom_clamped_by_route_maximum() {
        let mut entry = json!({"context_window": 128000, "max_context_window": 200000});
        apply_model_context(
            &mut entry,
            ModelContextPolicy::Custom { tokens: 500000 },
            None,
            None,
            None,
        );
        assert_eq!(entry["context_window"], 200000);
        assert_eq!(entry["auto_compact_token_limit"], 180_000);
    }

    #[test]
    fn short_on_model_without_window_does_nothing() {
        let mut unknown = json!({"slug": "unknown"});
        apply_model_context(&mut unknown, ModelContextPolicy::Extended, None, None, None);
        assert!(unknown.get("context_window").is_none());
    }

    #[test]
    fn auto_compact_is_ninety_percent_of_window() {
        let mut entry = json!({
            "context_window": 256000,
            "max_context_window": 256000,
            "effective_context_window_percent": 95
        });
        apply_model_context(&mut entry, ModelContextPolicy::Short, None, None, None);
        assert_eq!(entry["auto_compact_token_limit"], 230_400);
    }

    #[test]
    fn default_only_projection_is_idempotent_and_not_a_custom_ceiling() {
        let mut entry = json!({"context_window": 128000});
        apply_model_context(&mut entry, ModelContextPolicy::Short, None, None, None);
        let projected = entry.clone();
        apply_model_context(&mut entry, ModelContextPolicy::Short, None, None, None);
        assert_eq!(entry, projected);
        assert_eq!(entry["context_window"], 128000);
        assert_eq!(
            context_window_tokens(
                ModelContextPolicy::Custom { tokens: 250000 },
                Some(128000),
                None
            ),
            Some(250000)
        );
    }

    #[test]
    fn budget_resolver_handles_unknown_tiny_and_million_token_windows() {
        assert_eq!(
            context_window_tokens(ModelContextPolicy::Short, None, None),
            None
        );
        assert_eq!(
            context_window_tokens(ModelContextPolicy::Short, None, Some(1000000)),
            Some(250000)
        );
        assert_eq!(
            context_window_tokens(ModelContextPolicy::Short, None, Some(512)),
            Some(512)
        );
        assert_eq!(
            context_window_tokens(ModelContextPolicy::Extended, Some(128000), Some(64000)),
            Some(64000)
        );
    }

    #[test]
    fn published_windows_are_independent_of_policy_projection() {
        let windows = ModelContextWindows::from_entry(&json!({
            "context_window": 272000,
            "max_context_window": 1050000
        }));
        assert_eq!(
            windows.window_tokens(ModelContextPolicy::Short),
            Some(272000)
        );
        assert_eq!(
            windows.window_tokens(ModelContextPolicy::Extended),
            Some(1050000)
        );
        assert_eq!(
            windows.window_tokens(ModelContextPolicy::Custom { tokens: 500000 }),
            Some(500000)
        );
        assert_eq!(
            windows.window_tokens(ModelContextPolicy::Short),
            Some(272000)
        );
        assert_eq!(
            ModelContextWindows::from_entry(&json!({
                "context_window": 0, "max_context_window": -1
            })),
            ModelContextWindows::default()
        );
    }

    #[test]
    fn extended_falls_back_to_default_when_no_long_window() {
        let mut entry = json!({"context_window": 200000, "max_context_window": 200000});
        apply_model_context(&mut entry, ModelContextPolicy::Extended, None, None, None);
        assert_eq!(entry["context_window"], 200000);
        assert_eq!(entry["auto_compact_token_limit"], 180_000);
    }

    #[test]
    fn custom_validation_rejects_out_of_range() {
        assert!(ModelContextPolicy::Custom { tokens: 500 }
            .validate()
            .is_err());
        assert!(ModelContextPolicy::Custom { tokens: 3_000_000 }
            .validate()
            .is_err());
        assert!(ModelContextPolicy::Custom { tokens: 100_000 }
            .validate()
            .is_ok());
        assert!(ModelContextPolicy::Short.validate().is_ok());
    }

    #[test]
    fn non_text_models_do_not_support_text_context() {
        assert!(!model_supports_text_context(
            "gpt-image-2",
            Some("gpt-image"),
            &["text".into()],
        ));
        assert!(!model_supports_text_context(
            "dall-e-4",
            None,
            &["image".into()],
        ));
        assert!(!model_supports_text_context(
            "gemini-video",
            None,
            &["video".into()],
        ));
        assert!(!model_supports_text_context(
            "text-embedding-3-large",
            Some("embedding"),
            &[],
        ));
        assert!(model_supports_text_context(
            "vision",
            Some("multimodal"),
            &["text".into(), "image".into()],
        ));
        assert!(model_supports_text_context(
            "gpt-5.4",
            Some("gpt"),
            &["text".into()],
        ));
    }

    #[test]
    fn non_text_entries_do_not_receive_context_policy() {
        let mut entry = json!({
            "id": "gpt-image-2",
            "output_modalities": ["image"],
            "context_window": 128000,
            "max_context_window": 256000
        });
        apply_model_context(&mut entry, ModelContextPolicy::Extended, None, None, None);
        assert!(entry.get("auto_compact_token_limit").is_none());
        assert_eq!(entry["context_window"], 128000);
    }
}
