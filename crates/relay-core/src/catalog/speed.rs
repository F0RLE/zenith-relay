use super::{codex_model_is_picker_eligible, is_valid_model_token, reasoning_policy_key};
use crate::DefaultServiceTier;

/// Relay's request-speed policy, not upstream entitlement discovery. Every
/// OpenAI conversational model offers the same choices on accounts and API
/// sources. Existing family identity handles future versions and namespaces;
/// source tier lists, availability and cooldowns do not change these choices.
pub fn model_service_tiers(model: &str, provider: Option<&str>) -> &'static [DefaultServiceTier] {
    let model = model.trim();
    if !is_valid_model_token(model) {
        return &[];
    }
    let openai = provider
        .filter(|provider| !provider.trim().is_empty())
        .map_or_else(
            || reasoning_policy_key(model) == "group:openai",
            |provider| provider.eq_ignore_ascii_case("openai"),
        );
    if openai && codex_model_is_picker_eligible(model) {
        &[
            DefaultServiceTier::Standard,
            DefaultServiceTier::Fast,
            DefaultServiceTier::Ultrafast,
        ]
    } else {
        &[DefaultServiceTier::Standard]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_policy_uses_model_family_without_a_version_allowlist() {
        for model in [
            "gpt-future",
            "provider/GPT-future",
            "codex-future",
            "o99-synthetic",
        ] {
            assert_eq!(model_service_tiers(model, None).len(), 3, "{model}");
        }
        assert_eq!(model_service_tiers("future-name", Some("OpenAI")).len(), 3);
        for (model, provider) in [
            ("claude-future", None),
            ("gemini-future", None),
            ("grok-future", None),
            ("unknown", None),
            ("gpt-image-future", None),
            ("gpt-realtime-future", None),
            ("gpt-oss-synthetic", Some("other")),
        ] {
            assert_eq!(
                model_service_tiers(model, provider),
                [DefaultServiceTier::Standard]
            );
        }
        assert!(model_service_tiers(" ", None).is_empty());
    }
}
