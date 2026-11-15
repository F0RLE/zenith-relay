//! Native Images wire validation. Borrow image strings; never persist image data.
use crate::UsageEvent;
use crate::gateway::response::apply_usage;
pub(super) use crate::protocol::images::ImageEnvelope;
use serde_json::{Value, value::RawValue};

impl ImageEnvelope<'_> {
    pub(super) fn observe_usage(&self, event: &mut UsageEvent) {
        ImageUsageState::default().observe(event, self);
    }
}

#[derive(Default)]
pub(super) struct ImageUsageState {
    rendering_input: Option<u64>,
    rendering_output: Option<u64>,
    rendering_total: Option<u64>,
    rendering_reasoning: Option<u64>,
}

impl ImageUsageState {
    pub(super) fn observe(&mut self, event: &mut UsageEvent, envelope: &ImageEnvelope<'_>) {
        let parse = |raw: &RawValue| {
            serde_json::from_str::<Value>(raw.get())
                .ok()
                .filter(Value::is_object)
        };
        let rendering = envelope.usage.and_then(parse);
        let svg = envelope.svg_usage.and_then(parse);
        if rendering.is_none() && svg.is_none() {
            return;
        }
        // Restore the rendering snapshot before applying a new one: event totals
        // may already include SVG authoring from a preceding stream frame.
        event.input_tokens = self.rendering_input;
        event.output_tokens = self.rendering_output;
        event.total_tokens = self.rendering_total;
        event.reasoning_tokens = self.rendering_reasoning;
        if let Some(usage) = rendering {
            apply_usage(event, &usage);
            if !["input_tokens", "output_tokens", "total_tokens"]
                .iter()
                .any(|key| usage.get(key).and_then(Value::as_u64).is_some())
            {
                event.total_tokens = self.rendering_total;
            }
            let image_usage = event.image_usage.get_or_insert_with(Default::default);
            image_usage.observe(&usage);
            image_usage.cached_input_tokens = event
                .cached_input_tokens
                .or(image_usage.cached_input_tokens);
            self.rendering_input = event.input_tokens;
            self.rendering_output = event.output_tokens;
            self.rendering_total = event.total_tokens;
            self.rendering_reasoning = event.reasoning_tokens;
        }
        if let Some(usage) = svg {
            event
                .image_usage
                .get_or_insert_with(Default::default)
                .svg_usage
                .get_or_insert_with(Default::default)
                .observe(&usage);
        }
        if let Some(svg) = event
            .image_usage
            .as_ref()
            .and_then(|image| image.svg_usage.as_ref())
        {
            // Missing components remain unknown; do not invent a zero-cost stage.
            event.input_tokens = add_counts(self.rendering_input, svg.input_tokens);
            event.output_tokens = add_counts(self.rendering_output, svg.output_tokens);
            event.total_tokens = add_counts(self.rendering_total, svg.total_tokens);
            // SVG authoring does not report cache coverage. Its input is part of
            // the request total, so rendering-only cache cannot be its numerator.
            event.cached_input_tokens = None;
            event.reasoning_tokens = match (self.rendering_reasoning, svg.reasoning_tokens) {
                (Some(rendering), Some(authoring)) => rendering.checked_add(authoring),
                (rendering, authoring) => rendering.or(authoring),
            };
        }
    }
}

fn add_counts(left: Option<u64>, right: Option<u64>) -> Option<u64> {
    left.zip(right)
        .and_then(|(left, right)| left.checked_add(right))
}

pub(super) fn normalize_response(
    bytes: Vec<u8>,
    response_format: &str,
    requested_output_format: Option<&str>,
) -> Vec<u8> {
    if response_format != "url" {
        return bytes;
    }
    let Ok(mut payload) = serde_json::from_slice::<Value>(&bytes) else {
        return bytes;
    };
    let default_mime = image_mime_type(
        payload
            .get("output_format")
            .and_then(Value::as_str)
            .or(requested_output_format)
            .unwrap_or("png"),
    );
    if let Some(data) = payload.get_mut("data").and_then(Value::as_array_mut) {
        for image in data {
            let Some(object) = image.as_object_mut() else {
                continue;
            };
            if object
                .get("url")
                .and_then(Value::as_str)
                .is_some_and(|url| !url.trim().is_empty())
            {
                continue;
            }
            let mime = object
                .get("output_format")
                .and_then(Value::as_str)
                .map(image_mime_type)
                .unwrap_or(default_mime);
            if let Some(Value::String(base64)) = object.get("b64_json")
                && !base64.trim().is_empty()
            {
                let url = format!("data:{mime};base64,{base64}");
                object.remove("b64_json");
                object.insert("url".into(), Value::String(url));
            }
        }
    }
    serde_json::to_vec(&payload).unwrap_or(bytes)
}

fn image_mime_type(format: &str) -> &'static str {
    match format {
        "png" => "image/png",
        "jpeg" | "jpg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        // Preserve future formats without mislabeling their bytes as PNG.
        _ => "application/octet-stream",
    }
}
