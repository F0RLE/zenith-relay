use super::is_valid_model_token;
use crate::DefaultServiceTier;
use serde_json::Value;

/// Stable picker order for modes explicitly published by a model reference.
/// Standard is the baseline; unknown models do not acquire faster modes.
pub fn model_service_tiers(
    model: &str,
    declared: &[DefaultServiceTier],
) -> Vec<DefaultServiceTier> {
    if !is_valid_model_token(model.trim()) {
        return Vec::new();
    }
    [
        DefaultServiceTier::Standard,
        DefaultServiceTier::Fast,
        DefaultServiceTier::Ultrafast,
    ]
    .into_iter()
    .filter(|tier| *tier == DefaultServiceTier::Standard || declared.contains(tier))
    .collect()
}

pub(crate) fn reference_service_tiers(record: &Value) -> Vec<DefaultServiceTier> {
    let explicit = record
        .get("service_tiers")
        .and_then(Value::as_array)
        .or_else(|| {
            record
                .get("supported_service_tiers")
                .and_then(Value::as_array)
        });
    let explicit_names = explicit
        .into_iter()
        .flatten()
        .filter_map(|tier| tier.as_str().or_else(|| tier.get("id")?.as_str()));
    let modes = record
        .pointer("/experimental/modes")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter(|_| explicit.is_none())
        .filter_map(|(name, mode)| {
            mode.pointer("/provider/body/service_tier")
                .and_then(Value::as_str)
                .or(Some(name.as_str()))
        });
    explicit_names
        .chain(modes)
        .filter_map(|name| match name.trim().to_ascii_lowercase().as_str() {
            "standard" | "default" => Some(DefaultServiceTier::Standard),
            "fast" | "priority" => Some(DefaultServiceTier::Fast),
            "ultrafast" => Some(DefaultServiceTier::Ultrafast),
            _ => None,
        })
        .fold(Vec::new(), |mut tiers, tier| {
            if !tiers.contains(&tier) {
                tiers.push(tier);
            }
            tiers
        })
}

pub(crate) fn transport_supports_service_tier(
    tier: DefaultServiceTier,
    upstream: crate::WireApi,
    basis_points: bool,
    declared: Option<&[DefaultServiceTier]>,
) -> bool {
    let protocol_supports_tier = match tier {
        DefaultServiceTier::Standard => return true,
        DefaultServiceTier::Fast => {
            matches!(
                upstream,
                crate::WireApi::Responses | crate::WireApi::ChatCompletions
            )
        }
        DefaultServiceTier::Ultrafast => upstream == crate::WireApi::Responses,
    };
    !basis_points && protocol_supports_tier && declared.is_none_or(|tiers| tiers.contains(&tier))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_choices_require_explicit_reference_declarations() {
        let declared = [DefaultServiceTier::Fast, DefaultServiceTier::Ultrafast];
        for model in [
            "gpt-future",
            "provider/GPT-future",
            "codex-future",
            "o99-synthetic",
        ] {
            assert_eq!(model_service_tiers(model, &declared).len(), 3, "{model}");
        }
        for model in [
            "gpt-future",
            "claude-future",
            "gemini-future",
            "grok-future",
            "unknown",
            "gpt-image-future",
            "gpt-realtime-future",
            "gpt-oss-synthetic",
        ] {
            assert_eq!(
                model_service_tiers(model, &[]),
                [DefaultServiceTier::Standard]
            );
        }
        assert!(model_service_tiers(" ", &declared).is_empty());
    }
}
