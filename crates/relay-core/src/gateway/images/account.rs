use super::super::errors::{
    api_error_type, canonical_upstream_status, classify_upstream_error_value,
    rate_limit_body_hint_value, retryable_failure, upstream_failure_status,
    upstream_status_from_value, RateLimitBodyHint,
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
use axum::response::IntoResponse;
use axum::Json;
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

pub(super) fn direct_request_body(request: &PreparedImageRequest) -> Vec<u8> {
    if request
        .content_type
        .to_str()
        .is_ok_and(|value| value.to_ascii_lowercase().starts_with("application/json"))
    {
        let mut fields = request.fields.clone();
        fields.insert(
            "model".to_string(),
            Value::String(request.resolved_model.clone()),
        );
        return serde_json::to_vec(&fields).unwrap_or_else(|_| request.raw_body.to_vec());
    }
    request.raw_body.to_vec()
}

pub(super) fn build_account_request(
    request: &PreparedImageRequest,
    endpoint: ImageEndpoint,
    main_model: &str,
) -> Value {
    let mut tool = Map::new();
    tool.insert(
        "type".to_string(),
        Value::String("image_generation".to_string()),
    );
    tool.insert(
        "action".to_string(),
        Value::String(endpoint.action().to_string()),
    );
    tool.insert(
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
    for name in string_fields {
        if let Some(value) = request
            .fields
            .get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            tool.insert(name.to_string(), Value::String(value.to_string()));
        }
    }
    for name in ["n", "output_compression", "partial_images"] {
        if let Some(value) = request.fields.get(name).filter(|value| value.is_number()) {
            tool.insert(name.to_string(), value.clone());
        }
    }
    if let Some(mask) = request.mask_image.as_ref() {
        tool.insert("input_image_mask".to_string(), json!({"image_url": mask}));
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
        "tools": [Value::Object(tool)],
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
