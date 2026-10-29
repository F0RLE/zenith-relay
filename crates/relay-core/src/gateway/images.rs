use super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use crate::protocol::ClientWireApi;
use crate::{GatewayRuntime, WireApi};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderValue, Request, Response};
use serde_json::{Map, Value};
use std::sync::Arc;

mod account;
mod execute;
mod multipart;
mod native;
mod prepare;
#[cfg(test)]
mod tests;

const IMAGE_PROTOCOLS: &[WireApi] = &[WireApi::Responses, WireApi::ChatCompletions];
// Bounds apply to decoded bodies, not a potentially compressed Content-Length.
const MAX_IMAGE_BODY_BYTES: usize = 128 * 1024 * 1024;
const MAX_IMAGE_EVENT_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Eq, PartialEq)]
enum ImageEndpoint {
    Generations,
    Edits,
}

impl ImageEndpoint {
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

type ParsedImageFields = (Map<String, Value>, Vec<String>, Option<String>);

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
    let prepared = match prepare::prepare_request(&runtime, &key, &headers, body, endpoint).await {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    execute::execute_prepared(runtime, key, prepared, endpoint).await
}
