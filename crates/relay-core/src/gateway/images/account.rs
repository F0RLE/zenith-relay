use super::super::errors::{
    canonical_upstream_status, classify_upstream_error_value, rate_limit_body_hint_value,
    retryable_failure, upstream_failure_status, upstream_status_from_value, RateLimitBodyHint,
};
use super::super::now_ms;
use super::super::response::{usage_event, UsageAttempt};
use super::{ImageEndpoint, ImageFailure, PreparedImageRequest, TranslatedImageResponse};

mod response;
pub(super) use response::{
    image_capability_unavailable, image_error_response, translate_account_response,
};

use crate::error_codes;
use crate::protocol::sse_event_end;
use crate::runtime::{AuthenticatedKey, ExecutorRoute};
use crate::UsageEvent;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use serde_json::{json, Map, Value};
use std::time::{Instant, SystemTime};

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
        usage_event(
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
        )
    }
}

pub(super) fn direct_request_body(request: &PreparedImageRequest, source_model: &str) -> Vec<u8> {
    if request.content_type.to_str().is_ok_and(|content_type| {
        content_type
            .to_ascii_lowercase()
            .starts_with("application/json")
    }) {
        let mut request_fields = request.fields.clone();
        request_fields.insert("model".to_string(), Value::String(source_model.to_string()));
        // GPT Image models always return base64 image data. Older clients may
        // still send the legacy Image API response_format switch; forwarding
        // it makes current GPT Image providers reject an otherwise valid
        // request. Relay already normalizes the response to the requested
        // public format, so it is safe to omit this provider-incompatible
        // field for the new family.
        if crate::model_id_key(source_model).starts_with("gpt-image-") {
            request_fields.remove("response_format");
        }
        return serde_json::to_vec(&request_fields).unwrap_or_else(|_| request.raw_body.to_vec());
    }
    request.raw_body.to_vec()
}

pub(super) fn build_account_request(
    request: &PreparedImageRequest,
    endpoint: ImageEndpoint,
    main_model: &str,
) -> Value {
    let mut image_tool = Map::new();
    image_tool.insert(
        "type".to_string(),
        Value::String("image_generation".to_string()),
    );
    image_tool.insert(
        "action".to_string(),
        Value::String(endpoint.action().to_string()),
    );
    image_tool.insert(
        "model".to_string(),
        Value::String(request.resolved_model.clone()),
    );
    let mut string_fields = vec![
        "size",
        "quality",
        "background",
        "output_format",
        "moderation",
    ];
    if endpoint == ImageEndpoint::Edits {
        string_fields.push("input_fidelity");
    }
    for field_name in string_fields {
        if let Some(field_text) = request
            .fields
            .get(field_name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|field_text| !field_text.is_empty())
        {
            image_tool.insert(
                field_name.to_string(),
                Value::String(field_text.to_string()),
            );
        }
    }
    for field_name in ["n", "output_compression", "partial_images"] {
        if let Some(numeric_field) = request
            .fields
            .get(field_name)
            .filter(|field_value| field_value.is_number())
        {
            image_tool.insert(field_name.to_string(), numeric_field.clone());
        }
    }
    if let Some(mask) = request.mask_image.as_ref() {
        image_tool.insert("input_image_mask".to_string(), json!({"image_url": mask}));
    }

    let mut content = vec![json!({
        "type": "input_text",
        "text": request.fields.get("prompt").and_then(Value::as_str).unwrap_or_default(),
    })];
    content.extend(request.input_images.iter().map(|image| {
        json!({
            "type": "input_image",
            "image_url": image,
        })
    }));
    json!({
        "instructions": "",
        "stream": true,
        "reasoning": {"effort": "medium", "summary": "auto"},
        "parallel_tool_calls": true,
        "include": ["reasoning.encrypted_content"],
        "model": main_model,
        "store": false,
        "tool_choice": {"type": "image_generation"},
        "input": [{
            "type": "message",
            "role": "user",
            "content": content,
        }],
        "tools": [Value::Object(image_tool)],
    })
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
