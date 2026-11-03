use super::super::response::{UsageAttempt, usage_event};
use super::{ImageEndpoint, PreparedImageRequest};
use crate::runtime::{AuthenticatedKey, ExecutorRoute};
use crate::{UsageEvent, error_codes};
use axum::http::StatusCode;
use serde_json::{Value, json};
use std::time::Instant;

pub(super) struct ImageAttempt<'a> {
    pub(super) request_id: &'a str,
    pub(super) attempt: u16,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) route: &'a ExecutorRoute,
    pub(super) prepared: &'a PreparedImageRequest,
    pub(super) started: Instant,
}

impl ImageAttempt<'_> {
    pub(super) fn event(
        &self,
        success: bool,
        status: StatusCode,
        category: Option<String>,
    ) -> UsageEvent {
        let mut event = usage_event(
            UsageAttempt {
                request_id: self.request_id,
                attempt: self.attempt,
                local_key_id: &self.key.id,
                route: self.route,
                reasoning_effort: None,
                requested_model: &self.prepared.requested_model,
                tool_use: crate::ToolUseDiagnostics::default(),
            },
            success,
            status.as_u16(),
            category,
            self.started.elapsed().as_millis() as u64,
        );
        event.image_usage = Some(crate::usage::ImageUsage::default());
        if let Some(routing) = event.routing.as_mut() {
            routing.endpoint_kind = Some("images".to_owned());
        }
        event
    }
}

pub(super) fn image_capability_unavailable(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    text.contains("image generation is not enabled")
        || text.contains(error_codes::IMAGE_GENERATION_NOT_ENABLED)
}

pub(super) fn direct_request_body(request: &PreparedImageRequest, source_model: &str) -> Vec<u8> {
    if request
        .content_type
        .to_str()
        .is_ok_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
    {
        let mut fields = request.fields.clone();
        fields.insert("model".to_owned(), Value::String(source_model.to_owned()));
        // GPT Image always returns base64; legacy URL compatibility is local.
        if crate::model_id_key(source_model).starts_with("gpt-image-") {
            fields.remove("response_format");
        }
        return serde_json::to_vec(&fields).expect("image fields are JSON values");
    }
    request.raw_body.to_vec()
}

/// Native account Images does not invoke or bill a text-model tool call.
pub(super) fn direct_account_request_body(
    request: &PreparedImageRequest,
    endpoint: ImageEndpoint,
    source_model: &str,
) -> Vec<u8> {
    let mut fields = request.fields.clone();
    fields.insert("model".into(), Value::String(source_model.to_owned()));
    fields.remove("response_format");
    if endpoint == ImageEndpoint::Edits {
        fields.remove("image");
        fields.remove("image[]");
        fields.insert(
            "images".into(),
            Value::Array(
                request
                    .input_images
                    .iter()
                    .map(|image| json!({"image_url": image}))
                    .collect(),
            ),
        );
        if let Some(mask) = &request.mask_image {
            fields.insert("mask".into(), json!({"image_url": mask}));
        }
    }
    serde_json::to_vec(&fields).expect("image fields are JSON values")
}

pub(super) fn image_endpoint_url(
    mut responses_url: url::Url,
    endpoint: ImageEndpoint,
) -> Option<url::Url> {
    let mut segments = responses_url.path_segments_mut().ok()?;
    segments
        .pop_if_empty()
        .pop()
        .push("images")
        .push(match endpoint {
            ImageEndpoint::Generations => "generations",
            ImageEndpoint::Edits => "edits",
        });
    drop(segments);
    Some(responses_url)
}
