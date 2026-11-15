//! Bounded, opt-in metadata inspection; never changes image traffic or retries.
use super::PreparedImageRequest;
use crate::runtime::ExecutorRoute;
use crate::usage::{ImageDiagnostics, ImageOptionsDiagnostics, ImageOutputDiagnostics};
use crate::{GatewayRuntime, UsageEvent};
use axum::body::Bytes;
use serde::{
    Deserialize, Deserializer,
    de::{SeqAccess, Visitor},
};
use serde_json::{Value, value::RawValue};

mod headers;
#[cfg(test)]
mod tests;

const MAX_OUTPUTS: usize = 10;
const OPTION_KEYS: [&str; 6] = [
    "model",
    "size",
    "quality",
    "complexity",
    "output_format",
    "n",
];

pub(super) async fn observe_request(
    runtime: &GatewayRuntime,
    route: &mut ExecutorRoute,
    prepared: &PreparedImageRequest,
    body: &Bytes,
    content_type: &str,
) {
    if !runtime.diagnostics_enabled() || !route.diagnostics_enabled {
        return;
    }
    let Some(routing) = route.routing.as_mut() else {
        return;
    };
    let requested = requested_options(prepared);
    let sent = if content_type
        .to_ascii_lowercase()
        .starts_with("multipart/form-data")
    {
        multipart_options(body, content_type).await
    } else {
        serde_json::from_slice::<Observation<'_>>(body)
            .ok()
            .map(|value| value.options())
    };
    routing.images = Some(ImageDiagnostics {
        requested,
        sent,
        ..Default::default()
    });
}

fn requested_options(prepared: &PreparedImageRequest) -> ImageOptionsDiagnostics {
    let mut requested = ImageOptionsDiagnostics::default();
    for key in OPTION_KEYS {
        if let Some(value) = prepared.fields.get(key) {
            requested.observe(key, value);
        }
    }
    // Preparation has already resolved the public alias in `fields`.
    requested.observe("model", &Value::String(prepared.requested_model.clone()));
    requested
}

async fn multipart_options(body: &Bytes, content_type: &str) -> Option<ImageOptionsDiagnostics> {
    let boundary = multer::parse_boundary(content_type).ok()?;
    let mut parts = multer::Multipart::new(
        futures_util::stream::once(std::future::ready(Ok::<_, std::io::Error>(body.clone()))),
        boundary,
    );
    let mut options = ImageOptionsDiagnostics::default();
    while let Some(field) = parts.next_field().await.ok()? {
        let key = field.name().unwrap_or_default().to_owned();
        if !OPTION_KEYS.contains(&key.as_str()) || field.file_name().is_some() {
            continue;
        }
        let bytes = field.bytes().await.ok()?;
        if bytes.len() <= 128 {
            let text = std::str::from_utf8(&bytes).ok()?;
            let value = if key == "n" {
                serde_json::from_str(text).ok()?
            } else {
                Value::String(text.to_owned())
            };
            options.observe(&key, &value);
        }
    }
    Some(options)
}

/// Only completed SSE images call this function; previews are never probed.
pub(super) fn observe_response(runtime: &GatewayRuntime, event: &mut UsageEvent, bytes: &[u8]) {
    let Some(routing) = event.routing.as_mut() else {
        return;
    };
    if !runtime.diagnostics_enabled() {
        routing.images = None;
        return;
    }
    if let Some(diagnostics) = routing.images.as_mut() {
        inspect_response(diagnostics, bytes);
    }
}

fn inspect_response(diagnostics: &mut ImageDiagnostics, bytes: &[u8]) {
    let Ok(envelope) = serde_json::from_slice::<Observation<'_>>(bytes) else {
        return;
    };
    if diagnostics.reported.len() < MAX_OUTPUTS {
        diagnostics.reported.push(envelope.options());
    } else {
        diagnostics.truncated = true;
    }
    if let Some(data) = envelope.data {
        if let Ok(items) = serde_json::from_str::<OutputItems<'_>>(data.get()) {
            diagnostics.truncated |= items.truncated;
            for item in items.items {
                if let Ok(image) = serde_json::from_str::<Observation<'_>>(item.get()) {
                    observe_output(diagnostics, &image);
                }
            }
        }
    } else if envelope.b64_json.is_some() || envelope.url.is_some() {
        observe_output(diagnostics, &envelope);
    }
}

fn observe_output(diagnostics: &mut ImageDiagnostics, image: &Observation<'_>) {
    if diagnostics.outputs.len() >= MAX_OUTPUTS {
        diagnostics.truncated = true;
        return;
    }
    let (header_status, detected_format, dimensions) = headers::inspect(image.b64_json, image.url);
    let sent_size = diagnostics
        .sent
        .as_ref()
        .and_then(|sent| sent.size.as_deref())
        .and_then(headers::exact_size);
    diagnostics.outputs.push(ImageOutputDiagnostics {
        index: diagnostics.outputs.len() as u8,
        reported: image.options(),
        header_status,
        detected_format: detected_format.map(str::to_owned),
        dimensions,
        sent_size_matches: dimensions
            .zip(sent_size)
            .map(|(actual, sent)| actual == sent),
    });
}

// Raw fields keep invalid optional metadata observational: it cannot reject a
// successful generation, and large prompt/base64 strings are not copied.
#[derive(Deserialize)]
struct Observation<'a> {
    #[serde(default, borrow)]
    model: Option<&'a RawValue>,
    #[serde(default, borrow)]
    size: Option<&'a RawValue>,
    #[serde(default, borrow)]
    quality: Option<&'a RawValue>,
    #[serde(default, borrow)]
    complexity: Option<&'a RawValue>,
    #[serde(default, borrow)]
    output_format: Option<&'a RawValue>,
    #[serde(default, borrow)]
    n: Option<&'a RawValue>,
    #[serde(default, borrow)]
    data: Option<&'a RawValue>,
    #[serde(default, borrow)]
    b64_json: Option<&'a RawValue>,
    #[serde(default, borrow)]
    url: Option<&'a RawValue>,
}

impl Observation<'_> {
    fn options(&self) -> ImageOptionsDiagnostics {
        let mut options = ImageOptionsDiagnostics::default();
        for (key, raw) in OPTION_KEYS.into_iter().zip([
            self.model,
            self.size,
            self.quality,
            self.complexity,
            self.output_format,
            self.n,
        ]) {
            if let Some(raw) = raw.filter(|raw| raw.get().len() <= 128)
                && let Ok(value) = serde_json::from_str(raw.get())
            {
                options.observe(key, &value);
            }
        }
        options
    }
}

struct OutputItems<'a> {
    items: Vec<&'a RawValue>,
    truncated: bool,
}

impl<'de> Deserialize<'de> for OutputItems<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ItemsVisitor;
        impl<'de> Visitor<'de> for ItemsVisitor {
            type Value = OutputItems<'de>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("an image array")
            }
            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut items = Vec::new();
                let mut truncated = false;
                while let Some(raw) = sequence.next_element::<&'de RawValue>()? {
                    if items.len() < MAX_OUTPUTS {
                        items.push(raw)
                    } else {
                        truncated = true
                    }
                }
                Ok(OutputItems { items, truncated })
            }
        }
        deserializer.deserialize_seq(ItemsVisitor)
    }
}
