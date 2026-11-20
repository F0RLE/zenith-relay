//! Opt-in requested, sent and provider-reported image options.
use serde::{Deserialize, Serialize};

mod options;

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOptionsDiagnostics {
    /// Known public image model IDs only; custom identifiers are not retained.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub complexity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub n: Option<u8>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDimensions {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageHeaderStatus {
    Observed,
    UrlNotFetched,
    Missing,
    InvalidBase64,
    UnknownOrIncomplete,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOutputDiagnostics {
    pub index: u8,
    /// Per-image metadata, never filled from requested or envelope defaults.
    #[serde(default, skip_serializing_if = "ImageOptionsDiagnostics::is_empty")]
    pub reported: ImageOptionsDiagnostics,
    pub header_status: ImageHeaderStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detected_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<ImageDimensions>,
    /// Header dimensions versus the exact size sent, not a quality verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_size_matches: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageDiagnostics {
    pub requested: ImageOptionsDiagnostics,
    /// None means final payload inspection failed; no inferred replacement.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent: Option<ImageOptionsDiagnostics>,
    /// Buffered envelope or individual final SSE event metadata, capped at ten.
    #[serde(default)]
    pub reported: Vec<ImageOptionsDiagnostics>,
    #[serde(default)]
    pub outputs: Vec<ImageOutputDiagnostics>,
    #[serde(default)]
    pub truncated: bool,
}

impl ImageDiagnostics {
    /// Reapply the allowlist when exporting historical or externally loaded data.
    pub fn sanitized(&self) -> Self {
        Self {
            requested: self.requested.sanitized(),
            sent: self.sent.as_ref().map(ImageOptionsDiagnostics::sanitized),
            reported: self
                .reported
                .iter()
                .take(10)
                .map(ImageOptionsDiagnostics::sanitized)
                .collect(),
            outputs: self
                .outputs
                .iter()
                .take(10)
                .map(|output| ImageOutputDiagnostics {
                    index: output.index,
                    reported: output.reported.sanitized(),
                    header_status: output.header_status,
                    detected_format: output
                        .detected_format
                        .as_deref()
                        .filter(|format| matches!(*format, "png" | "gif" | "webp" | "jpeg"))
                        .map(str::to_owned),
                    dimensions: output.dimensions,
                    sent_size_matches: output.sent_size_matches,
                })
                .collect(),
            truncated: self.truncated || self.reported.len() > 10 || self.outputs.len() > 10,
        }
    }
}
