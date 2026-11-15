use super::{ImageDimensions, ImageOptionsDiagnostics};
use serde_json::Value;

impl ImageDimensions {
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let (width, height) = text.split_once('x')?;
        if ![width, height]
            .into_iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
        {
            return None;
        }
        let width = width.parse().ok()?;
        let height = height.parse().ok()?;
        (width > 0 && height > 0).then_some(Self { width, height })
    }
}

impl ImageOptionsDiagnostics {
    pub(crate) fn observe(&mut self, key: &str, value: &Value) {
        let text = value.as_str().filter(|text| text.len() <= 128);
        match key {
            "model" => {
                self.model = text
                    .filter(|text| {
                        matches!(
                            *text,
                            "dall-e-2"
                                | "dall-e-3"
                                | "gpt-image-1"
                                | "gpt-image-1-mini"
                                | "gpt-image-1.5"
                                | "gpt-image-2"
                                | "gpt-image-2-codex"
                                | "gpt-image-2.5-sunburst"
                                | "gpt-image-2.5-flare"
                                | "gpt-image-2.6-sunburst"
                                | "gpt-image-2.6-flare"
                        )
                    })
                    .map(str::to_owned)
            }
            "size" => {
                self.size = text
                    .filter(|text| *text == "auto" || ImageDimensions::parse(text).is_some())
                    .map(str::to_owned)
            }
            "quality" => {
                self.quality = text
                    .filter(|text| {
                        matches!(
                            *text,
                            "auto" | "low" | "medium" | "high" | "max" | "standard" | "hd"
                        )
                    })
                    .map(str::to_owned)
            }
            "complexity" => {
                self.complexity = text
                    .filter(|text| matches!(*text, "auto" | "low" | "medium" | "high"))
                    .map(str::to_owned)
            }
            "output_format" => {
                self.output_format = text
                    .filter(|text| {
                        matches!(
                            *text,
                            "png" | "jpeg" | "jpg" | "webp" | "gif" | "svg" | "auto"
                        )
                    })
                    .map(str::to_owned)
            }
            "n" => {
                self.n = value
                    .as_u64()
                    .filter(|count| (1..=10).contains(count))
                    .map(|count| count as u8)
            }
            _ => {}
        }
    }

    pub(super) fn sanitized(&self) -> Self {
        let mut safe = Self::default();
        for (key, value) in [
            ("model", &self.model),
            ("size", &self.size),
            ("quality", &self.quality),
            ("complexity", &self.complexity),
            ("output_format", &self.output_format),
        ] {
            if let Some(value) = value.as_ref().filter(|value| value.len() <= 128) {
                safe.observe(key, &Value::String(value.clone()));
            }
        }
        safe.n = self.n.filter(|count| (1..=10).contains(count));
        safe
    }
}
