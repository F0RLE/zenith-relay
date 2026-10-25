//! Native Images wire validation. Borrow image strings; never persist image data.
use crate::gateway::response::apply_usage;
use crate::UsageEvent;
use serde::Deserialize;
use serde_json::{value::RawValue, Value};
use std::borrow::Cow;

#[derive(Default, Deserialize)]
pub(super) struct ImageEnvelope<'a> {
    #[serde(default, borrow)]
    pub data: Vec<ImageItem<'a>>,
    #[serde(default, borrow)]
    pub b64_json: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    pub usage: Option<&'a RawValue>,
    #[serde(default, borrow)]
    pub error: Option<&'a RawValue>,
    #[serde(default, borrow, rename = "type")]
    pub kind: Option<Cow<'a, str>>,
}

#[derive(Default, Deserialize)]
pub(super) struct ImageItem<'a> {
    #[serde(default, borrow)]
    b64_json: Option<Cow<'a, str>>,
    #[serde(default, borrow)]
    url: Option<Cow<'a, str>>,
}

impl ImageEnvelope<'_> {
    pub(super) fn output_count(&self) -> usize {
        self.data
            .iter()
            .filter(|image| {
                image
                    .b64_json
                    .as_deref()
                    .or(image.url.as_deref())
                    .is_some_and(|data| !data.is_empty())
            })
            .count()
    }

    pub(super) fn observe_usage(&self, event: &mut UsageEvent) {
        if let Some(usage) = self
            .usage
            .and_then(|raw| serde_json::from_str::<Value>(raw.get()).ok())
        {
            apply_usage(event, &usage);
            event
                .image_usage
                .get_or_insert_with(Default::default)
                .observe(&usage);
        }
    }
}

pub(super) fn normalize_response(bytes: Vec<u8>, response_format: &str) -> Vec<u8> {
    if response_format != "url" {
        return bytes;
    }
    let Ok(mut payload) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes;
    };
    let format = payload
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("png");
    let mime = match format {
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    };
    if let Some(data) = payload.get_mut("data").and_then(Value::as_array_mut) {
        for image in data {
            if let Some(object) = image.as_object_mut() {
                if let Some(Value::String(base64)) = object.remove("b64_json") {
                    object.insert(
                        "url".into(),
                        Value::String(format!("data:{mime};base64,{base64}")),
                    );
                }
            }
        }
    }
    serde_json::to_vec(&payload).unwrap_or(bytes)
}
