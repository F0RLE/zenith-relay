use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Modality subsets of ordinary token totals, never additional consumption.
/// No image bytes, prompts, URLs, or inferred account credit costs are retained.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageUsage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_text_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_image_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_text_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_image_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_image_tokens: Option<u64>,
    #[serde(default)]
    pub generated_images: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_details_consistent: Option<bool>,
}

impl ImageUsage {
    pub(crate) fn observe(&mut self, usage: &Value) {
        // Usage events are cumulative snapshots. Missing fields do not erase
        // known observations; repeated completion snapshots are not summed.
        for (target, path) in [
            (
                &mut self.input_text_tokens,
                "/input_tokens_details/text_tokens",
            ),
            (
                &mut self.input_image_tokens,
                "/input_tokens_details/image_tokens",
            ),
            (
                &mut self.cached_text_tokens,
                "/input_tokens_details/cached_tokens_details/text_tokens",
            ),
            (
                &mut self.cached_image_tokens,
                "/input_tokens_details/cached_tokens_details/image_tokens",
            ),
            (&mut self.output_image_tokens, "/output_tokens"),
        ] {
            if let Some(value) = usage.pointer(path).and_then(Value::as_u64) {
                *target = Some(value);
            }
        }
        if let (Some(total), Some(text), Some(image)) = (
            usage.get("input_tokens").and_then(Value::as_u64),
            self.input_text_tokens,
            self.input_image_tokens,
        ) {
            self.input_details_consistent = Some(
                text.checked_add(image) == Some(total)
                    && self.cached_text_tokens.is_none_or(|cached| cached <= text)
                    && self
                        .cached_image_tokens
                        .is_none_or(|cached| cached <= image),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn image_subsets_are_snapshots_and_unknown_is_not_zero() {
        let mut usage = ImageUsage::default();
        assert_eq!(usage.input_text_tokens, None);
        let snapshot = json!({"input_tokens": 10, "output_tokens": 6,
            "input_tokens_details": {"text_tokens": 3, "image_tokens": 7}});
        usage.observe(&snapshot);
        usage.observe(&snapshot);
        usage.observe(&json!({}));
        assert_eq!(usage.input_text_tokens, Some(3));
        assert_eq!(usage.input_image_tokens, Some(7));
        assert_eq!(usage.output_image_tokens, Some(6));
        assert_eq!(usage.input_details_consistent, Some(true));
        usage.observe(&json!({"input_tokens": 11}));
        assert_eq!(usage.input_details_consistent, Some(false));
    }
}
