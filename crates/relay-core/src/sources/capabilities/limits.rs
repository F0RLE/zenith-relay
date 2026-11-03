use crate::model_metadata::ModelCapabilities;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Endpoint restrictions, not canonical model capabilities or a native
/// account's conversation window. Missing/invalid values stay unknown.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct ModelTokenLimits {
    pub context: Option<u64>,
    pub input: Option<u64>,
    pub output: Option<u64>,
}

impl ModelTokenLimits {
    pub(super) fn from_catalog(model: &Value) -> Self {
        // Accept documented numeric fields, not guesses from names, prices,
        // generation defaults (max_tokens), or participant capability flags.
        let limit = |aliases: &[&str]| {
            let mut found = None;
            for value in aliases
                .iter()
                .filter_map(|key| model.get(*key))
                .filter(|v| !v.is_null())
            {
                let parsed = value.as_u64().filter(|value| *value > 0)?;
                if found.is_some_and(|previous| previous != parsed) {
                    return None;
                }
                found = Some(parsed);
            }
            found
        };
        Self {
            context: limit(&["contextWindow", "context_window", "context_length"]),
            input: limit(&["maxInputTokens", "max_input_tokens", "inputTokenLimit"]),
            output: limit(&["maxOutputTokens", "max_output_tokens", "outputTokenLimit"]),
        }
        .validated()
    }

    pub(crate) fn validated(self) -> Self {
        let context = self.context.filter(|value| *value > 0);
        let within_context = |value: &u64| *value > 0 && context.is_none_or(|max| *value <= max);
        Self {
            context,
            input: self.input.filter(within_context),
            output: self.output.filter(within_context),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The first ingestion stage may fill unknown reference limits or narrow
    /// them, but cannot enlarge a known maximum or mutate the shared catalog.
    pub(crate) fn constrain(self, capabilities: &mut ModelCapabilities) {
        let limits = self.validated();
        let minimum = |left: Option<u64>, right: Option<u64>| {
            left.into_iter()
                .chain(right)
                .filter(|value| *value > 0)
                .min()
        };
        capabilities.context_limit = minimum(capabilities.context_limit, limits.context);
        capabilities.input_limit = minimum(capabilities.input_limit, limits.input)
            .map(|limit| limit.min(capabilities.context_limit.unwrap_or(u64::MAX)));
        capabilities.output_limit = minimum(capabilities.output_limit, limits.output)
            .map(|limit| limit.min(capabilities.context_limit.unwrap_or(u64::MAX)));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{CapabilityStatus, SourceProtocolConfig, WireApi, catalog_capabilities};
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_limits_accept_only_unambiguous_positive_integers() {
        let limits = ModelTokenLimits::from_catalog(&json!({
            "contextWindow":1000, "maxInputTokens":800, "maxOutputTokens":200
        }));
        assert_eq!(
            limits,
            ModelTokenLimits {
                context: Some(1000),
                input: Some(800),
                output: Some(200)
            }
        );
        for invalid in [
            json!(0),
            json!(-1),
            json!(1.5),
            json!("100"),
            json!(true),
            json!({}),
        ] {
            assert_eq!(
                ModelTokenLimits::from_catalog(&json!({"maxOutputTokens":invalid})).output,
                None
            );
        }
        assert_eq!(
            ModelTokenLimits::from_catalog(&json!({
                "maxOutputTokens":100, "max_output_tokens":200
            }))
            .output,
            None
        );
        assert_eq!(
            ModelTokenLimits::from_catalog(&json!({
                "contextWindow":100, "maxInputTokens":101, "maxOutputTokens":100
            })),
            ModelTokenLimits {
                context: Some(100),
                input: None,
                output: Some(100)
            }
        );
        assert!(ModelTokenLimits::from_catalog(&json!({"max_tokens":100})).is_empty());
    }

    #[test]
    fn gemini_input_and_output_limits_do_not_invent_a_combined_window() {
        let limits = ModelTokenLimits::from_catalog(&json!({
            "inputTokenLimit":1000, "outputTokenLimit":200
        }));
        assert_eq!(
            limits,
            ModelTokenLimits {
                context: None,
                input: Some(1000),
                output: Some(200)
            }
        );
    }

    #[test]
    fn route_limits_narrow_or_fill_without_inflating_reference_limits() {
        let reference = ModelCapabilities {
            context_limit: Some(1000),
            input_limit: Some(900),
            output_limit: None,
            ..Default::default()
        };
        let mut effective = reference.clone();
        ModelTokenLimits {
            context: Some(800),
            input: Some(2000),
            output: Some(200),
        }
        .constrain(&mut effective);
        assert_eq!(
            (
                effective.context_limit,
                effective.input_limit,
                effective.output_limit
            ),
            (Some(800), Some(800), Some(200))
        );
        assert_eq!(reference.context_limit, Some(1000));
        ModelTokenLimits {
            context: Some(2000),
            input: Some(1800),
            output: Some(400),
        }
        .constrain(&mut effective);
        assert_eq!(
            (
                effective.context_limit,
                effective.input_limit,
                effective.output_limit
            ),
            (Some(800), Some(800), Some(200))
        );
    }

    #[test]
    fn numeric_catalog_limits_do_not_declare_endpoints_or_semantic_features() {
        let observations = catalog_capabilities(
            &json!({"data":[{
                "id":"synthetic-model", "maxOutputTokens":100,
                "reasoningEffortModes":["high"], "capabilities":{"tools":true}
            }]}),
            1,
        );
        assert!(!observations.is_empty());
        for observation in &observations {
            assert_eq!(observation.status, CapabilityStatus::Unknown);
            assert!(!observation.explicitly_declares_endpoint());
            assert!(observation.features.is_empty());
            assert!(observation.reasoning_efforts.is_empty());
            assert_eq!(observation.token_limits.output, Some(100));
        }
        let base = "https://example.invalid/v1/responses";
        let mut config = SourceProtocolConfig::automatic(base);
        config.merge_catalog(observations);
        let models = vec!["synthetic-model".to_owned()];
        let resolved = config.effective_capabilities(base, &models);
        let responses = resolved
            .iter()
            .find(|cap| cap.upstream_wire_api == WireApi::Responses)
            .unwrap();
        assert_eq!(responses.status, CapabilityStatus::Declared);
        assert_eq!(responses.token_limits.output, Some(100));
        assert!(
            resolved
                .iter()
                .filter(|cap| cap.status.available())
                .all(|cap| cap.upstream_wire_api == WireApi::Responses)
        );
        let restored: SourceProtocolConfig =
            serde_json::from_value(serde_json::to_value(&config).unwrap()).unwrap();
        assert_eq!(restored, config);
        // A fresh complete catalog without the field withdraws the restriction.
        config.merge_catalog(catalog_capabilities(
            &json!({"data":[{"id":"synthetic-model"}]}),
            2,
        ));
        assert!(
            config
                .effective_capabilities(base, &models)
                .iter()
                .all(|cap| cap.token_limits.is_empty())
        );
    }

    #[test]
    fn endpoint_limit_evidence_is_scoped_to_the_exact_model() {
        let base = "https://example.invalid/v1/responses";
        let mut config = SourceProtocolConfig::automatic(base);
        config.merge_catalog(catalog_capabilities(&json!({"data":[{
            "id":"provider-a/model", "supported_endpoint_types":["responses"], "maxOutputTokens":100
        }, {"id":"provider-b/model", "supported_endpoint_types":["responses"], "maxOutputTokens":200}]}), 1));
        let effective = config.effective_capabilities(
            base,
            &[
                "provider-a/model".into(),
                "provider-b/model".into(),
                "model".into(),
            ],
        );
        for (model, maximum) in [
            ("provider-a/model", Some(100)),
            ("provider-b/model", Some(200)),
            ("model", None),
        ] {
            let route = effective
                .iter()
                .find(|cap| cap.model_id == model && cap.upstream_wire_api == WireApi::Responses)
                .unwrap();
            assert_eq!(route.token_limits.output, maximum);
        }
    }
}
