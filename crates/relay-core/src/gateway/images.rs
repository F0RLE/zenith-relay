use super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::errors::RateLimitBodyHint;
use crate::protocol::ClientWireApi;
use crate::{GatewayRuntime, WireApi};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderValue, Request, Response, StatusCode};
use serde_json::{Map, Value};
use std::sync::Arc;

mod account;
mod execute;
mod prepare;

use execute::execute_prepared;
use prepare::prepare_request;

const IMAGE_PROTOCOLS: &[WireApi] = &[WireApi::Responses, WireApi::ChatCompletions];

#[derive(Clone, Copy, Eq, PartialEq)]
enum ImageEndpoint {
    Generations,
    Edits,
}

impl ImageEndpoint {
    fn action(self) -> &'static str {
        match self {
            Self::Generations => "generate",
            Self::Edits => "edit",
        }
    }

    fn stream_prefix(self) -> &'static str {
        match self {
            Self::Generations => "image_generation",
            Self::Edits => "image_edit",
        }
    }
}

struct PreparedImageRequest {
    requested_model: String,
    resolved_model: String,
    fields: Map<String, Value>,
    input_images: Vec<String>,
    mask_image: Option<String>,
    raw_body: Bytes,
    content_type: HeaderValue,
    stream: bool,
    response_format: String,
    client_context_id: Option<String>,
}

#[derive(Debug)]
struct TranslatedImageResponse {
    json: Vec<u8>,
    stream: Vec<u8>,
    usage: Option<Value>,
}

type ParsedImageFields = (Map<String, Value>, Vec<String>, Option<String>);

#[derive(Debug)]
struct ImageFailure {
    upstream_error: Option<Box<crate::usage::UpstreamErrorDetails>>,
    status: StatusCode,
    category: &'static str,
    code: String,
    message: String,
    retryable: bool,
    cooldown_hint: RateLimitBodyHint,
}

pub(super) async fn generations(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute(runtime, request, ImageEndpoint::Generations).await
}

pub(super) async fn edits(
    State(runtime): State<Arc<GatewayRuntime>>,
    request: Request<Body>,
) -> Response<Body> {
    execute(runtime, request, ImageEndpoint::Edits).await
}

async fn execute(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    endpoint: ImageEndpoint,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    if !runtime.allows_client_wire_api(&key, ClientWireApi::ChatCompletions) {
        return client_api_forbidden();
    }
    let prepared = match prepare_request(&runtime, &key, &headers, body, endpoint).await {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    execute_prepared(runtime, key, prepared, endpoint).await
}

#[cfg(test)]
mod tests {
    use super::account::{build_account_request, translate_account_response};
    use super::prepare::parse_multipart;
    use super::*;
    use crate::runtime::IMAGE_API_MODEL;
    use axum::http::StatusCode;
    use serde_json::json;

    #[test]
    fn account_request_keeps_image_options_optional() {
        let request = PreparedImageRequest {
            requested_model: IMAGE_API_MODEL.to_string(),
            resolved_model: IMAGE_API_MODEL.to_string(),
            fields: serde_json::from_value(json!({
                "model": IMAGE_API_MODEL,
                "prompt": "draw",
                "quality": "low"
            }))
            .unwrap(),
            input_images: Vec::new(),
            mask_image: None,
            raw_body: Bytes::new(),
            content_type: HeaderValue::from_static("application/json"),
            stream: false,
            response_format: "b64_json".to_string(),
            client_context_id: None,
        };
        let body = build_account_request(&request, ImageEndpoint::Generations, "gpt-5.4-mini");
        assert_eq!(body["model"], "gpt-5.4-mini");
        assert_eq!(body["tools"][0]["model"], IMAGE_API_MODEL);
        assert_eq!(body["tools"][0]["quality"], "low");
        assert!(body["tools"][0].get("size").is_none());
    }

    #[test]
    fn completed_response_becomes_images_api_payload() {
        for ending in ["\n", "\r\n", "\r"] {
            let frame = "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"created_at\":7,\"output\":[{\"type\":\"image_generation_call\",\"result\":\"aW1hZ2U=\",\"output_format\":\"png\"}],\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n".replace('\n', ending);
            let translated =
                translate_account_response(frame.as_bytes(), "b64_json", "image_generation")
                    .unwrap();
            let body: Value = serde_json::from_slice(&translated.json).unwrap();
            assert_eq!(body["created"], 7);
            assert_eq!(body["data"][0]["b64_json"], "aW1hZ2U=");
            assert!(String::from_utf8(translated.stream)
                .unwrap()
                .contains("image_generation.completed"));
        }
    }

    #[test]
    fn image_user_error_is_not_retryable() {
        let failure = translate_account_response(
            b"data: {\"type\":\"error\",\"error\":{\"type\":\"image_generation_user_error\",\"code\":\"moderation_blocked\",\"message\":\"rejected\"}}\n\n",
            "b64_json",
            "image_generation",
        )
        .unwrap_err();
        assert_eq!(failure.status, StatusCode::BAD_REQUEST);
        assert!(!failure.retryable);
    }

    #[test]
    fn image_usage_limit_keeps_the_provider_reset() {
        let failure = translate_account_response(
            b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"usage_limit_reached\",\"resets_in_seconds\":12}}}\n\n",
            "b64_json",
            "image_generation",
        )
        .unwrap_err();
        assert_eq!(failure.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(failure.category, "upstream_quota_exhausted");
        assert!(failure.retryable);
        assert_eq!(failure.cooldown_hint.retry_after_ms, Some(12_000));
        assert!(failure.cooldown_hint.global);
    }

    #[test]
    fn done_with_incomplete_status_is_not_a_successful_image() {
        let failure = translate_account_response(
            br#"data: {"type":"response.done","response":{"status":"incomplete","incomplete_details":{"reason":"content_filter"},"output":[{"type":"image_generation_call","result":"aW1hZ2U="}]}}

"#,
            "b64_json",
            "image_generation",
        )
        .unwrap_err();
        assert_eq!(failure.code, crate::error_codes::RESPONSE_INCOMPLETE);
        assert!(failure.message.contains("content_filter"));
    }

    #[test]
    fn unknown_done_status_and_truncated_partials_stay_incomplete() {
        for body in [
            br#"data: {"type":"response.done","response":{"status":"queued","output":[{"type":"image_generation_call","result":"aW1hZ2U="}]}}

"#
            .as_slice(),
            br#"data: {"type":"response.image_generation_call.partial_image","partial_image_b64":"aaa","partial_image_index":0}

"#
            .as_slice(),
        ] {
            let failure = translate_account_response(body, "b64_json", "image_generation").unwrap_err();
            assert_eq!(failure.category, crate::error_codes::STREAM_INCOMPLETE);
        }
    }

    #[tokio::test]
    async fn multipart_edit_parses_multiple_images_and_mask() {
        let boundary = "zenith-test-boundary";
        let body = Bytes::from(format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nedit\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; filename=\"a.png\"\r\nContent-Type: image/png\r\n\r\nPNG-A\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; filename=\"b.png\"\r\nContent-Type: image/png\r\n\r\nPNG-B\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"mask\"; filename=\"mask.png\"\r\nContent-Type: image/png\r\n\r\nMASK\r\n--{boundary}--\r\n"
        ));
        let (fields, images, mask) =
            parse_multipart(&format!("multipart/form-data; boundary={boundary}"), body)
                .await
                .unwrap();
        assert_eq!(fields["prompt"], "edit");
        assert_eq!(images.len(), 2);
        assert!(images
            .iter()
            .all(|image| image.starts_with("data:image/png;base64,")));
        assert!(mask.unwrap().starts_with("data:image/png;base64,"));
    }
}
