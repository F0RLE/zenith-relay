//! Borrowed native Images envelopes shared by dispatch and result validation.
use serde::Deserialize;
use serde_json::{Value, value::RawValue};
use std::borrow::Cow;

pub(crate) const MAX_IMAGE_BODY_BYTES: usize = 128 * 1024 * 1024;

#[derive(Default, Deserialize)]
pub(crate) struct ImageEnvelope<'a> {
    #[serde(default, borrow)]
    data: Vec<ImageItem<'a>>,
    #[serde(default, borrow)]
    pub b64_json: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    pub usage: Option<&'a RawValue>,
    #[serde(default, borrow)]
    pub svg_usage: Option<&'a RawValue>,
    #[serde(default, borrow)]
    pub error: Option<&'a RawValue>,
    #[serde(default, borrow, rename = "type")]
    pub kind: Option<Cow<'a, str>>,
}

#[derive(Default, Deserialize)]
struct ImageItem<'a> {
    #[serde(default, borrow)]
    b64_json: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    url: Option<Cow<'a, str>>,
}

impl ImageEnvelope<'_> {
    pub(crate) fn has_generation_evidence(&self) -> bool {
        self.output_count() > 0
            || self
                .b64_json
                .as_deref()
                .is_some_and(|data| !data.trim().is_empty())
            || [self.usage, self.svg_usage]
                .into_iter()
                .flatten()
                .any(|raw| {
                    serde_json::from_str::<Value>(raw.get()).is_ok_and(|usage| {
                        ["output_tokens", "reasoning_tokens"]
                            .into_iter()
                            .any(|key| {
                                usage
                                    .get(key)
                                    .and_then(Value::as_u64)
                                    .is_some_and(|count| count > 0)
                            })
                            || usage
                                .pointer("/output_tokens_details/reasoning_tokens")
                                .and_then(Value::as_u64)
                                .is_some_and(|count| count > 0)
                    })
                })
    }

    pub(crate) fn output_count(&self) -> usize {
        self.data
            .iter()
            .filter(|image| {
                image
                    .b64_json
                    .as_deref()
                    .filter(|data| !data.trim().is_empty())
                    .or(image.url.as_deref())
                    .is_some_and(|data| !data.trim().is_empty())
            })
            .count()
    }
}
