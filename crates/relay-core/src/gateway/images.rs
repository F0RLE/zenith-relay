use super::*;
use crate::runtime::IMAGE_API_MODEL;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::stream;
use multer::{Constraints, Multipart, SizeLimit};
use std::io;

const MAX_IMAGE_REQUEST_BODY_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_RESPONSE_BODY_BYTES: usize = 64 * 1024 * 1024;
const MAX_IMAGE_UPLOAD_BYTES: u64 = 20 * 1024 * 1024;
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
    status: StatusCode,
    category: &'static str,
    code: String,
    message: String,
    retryable: bool,
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
    let prepared = match prepare_request(&runtime, &key, &headers, body, endpoint).await {
        Ok(prepared) => prepared,
        Err(response) => return response,
    };
    execute_prepared(runtime, key, prepared, endpoint).await
}

async fn prepare_request(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    headers: &HeaderMap,
    body: Body,
    endpoint: ImageEndpoint,
) -> Result<PreparedImageRequest, Response<Body>> {
    let raw_body = axum::body::to_bytes(body, MAX_IMAGE_REQUEST_BODY_BYTES)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                "image request body exceeds 64 MiB",
                "request_too_large",
            )
        })?;
    let content_type = headers
        .get(CONTENT_TYPE)
        .cloned()
        .unwrap_or_else(|| HeaderValue::from_static("application/json"));
    let content_type_text = content_type.to_str().unwrap_or_default();
    let (mut fields, input_images, mask_image) = if endpoint == ImageEndpoint::Edits
        && content_type_text
            .to_ascii_lowercase()
            .starts_with("multipart/form-data")
    {
        parse_multipart(content_type_text, raw_body.clone()).await?
    } else {
        parse_json(&raw_body, endpoint)?
    };

    let prompt = fields
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|prompt| !prompt.is_empty());
    if prompt.is_none() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "prompt must be a non-empty string",
            "invalid_request",
        ));
    }
    if endpoint == ImageEndpoint::Edits && input_images.is_empty() {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "image edits require at least one image",
            "invalid_request",
        ));
    }

    let requested_model = fields
        .get("model")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| match key.model_prefix.as_deref() {
            Some(prefix) => format!("{prefix}/{IMAGE_API_MODEL}"),
            None => IMAGE_API_MODEL.to_string(),
        });
    let visible = runtime.visible_models(key, IMAGE_PROTOCOLS, now_ms());
    if !visible
        .iter()
        .any(|model| model.eq_ignore_ascii_case(&requested_model))
    {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        ));
    }
    let Some(resolved_model) = runtime.resolve_model(key, &requested_model) else {
        return Err(api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        ));
    };
    if !resolved_model.eq_ignore_ascii_case(IMAGE_API_MODEL) {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "images endpoints require gpt-image-2",
            "invalid_image_model",
        ));
    }
    fields.insert("model".to_string(), Value::String(resolved_model.clone()));

    let stream = match fields.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "stream must be a boolean",
                "invalid_request",
            ))
        }
        None => false,
    };
    let response_format = fields
        .get("response_format")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|format| !format.is_empty())
        .unwrap_or("b64_json")
        .to_ascii_lowercase();
    if !matches!(response_format.as_str(), "b64_json" | "url") {
        return Err(api_error(
            StatusCode::BAD_REQUEST,
            "response_format must be b64_json or url",
            "invalid_request",
        ));
    }

    Ok(PreparedImageRequest {
        requested_model,
        resolved_model,
        fields,
        input_images,
        mask_image,
        raw_body,
        content_type,
        stream,
        response_format,
    })
}

#[allow(clippy::result_large_err)]
fn parse_json(body: &[u8], endpoint: ImageEndpoint) -> Result<ParsedImageFields, Response<Body>> {
    let fields = match serde_json::from_slice(body) {
        Ok(Value::Object(fields)) => fields,
        _ => {
            return Err(api_error(
                StatusCode::BAD_REQUEST,
                "request body must be a JSON object",
                "invalid_request",
            ))
        }
    };
    if endpoint == ImageEndpoint::Generations {
        return Ok((fields, Vec::new(), None));
    }

    let mut images = Vec::new();
    if let Some(image) = fields.get("image").and_then(Value::as_str) {
        push_non_empty(&mut images, image);
    }
    if let Some(values) = fields.get("images").and_then(Value::as_array) {
        for image in values {
            if let Some(url) = image
                .get("image_url")
                .and_then(Value::as_str)
                .or_else(|| image.as_str())
            {
                push_non_empty(&mut images, url);
            }
        }
    }
    let mask = fields.get("mask").and_then(|mask| {
        mask.get("image_url")
            .and_then(Value::as_str)
            .or_else(|| mask.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    });
    Ok((fields, images, mask))
}

#[allow(clippy::result_large_err)]
async fn parse_multipart(
    content_type: &str,
    body: Bytes,
) -> Result<ParsedImageFields, Response<Body>> {
    let boundary = multer::parse_boundary(content_type).map_err(|_| {
        api_error(
            StatusCode::BAD_REQUEST,
            "multipart boundary is invalid",
            "invalid_request",
        )
    })?;
    let size_limit = SizeLimit::new()
        .whole_stream(MAX_IMAGE_REQUEST_BODY_BYTES as u64)
        .per_field(MAX_IMAGE_UPLOAD_BYTES);
    let constraints = Constraints::new().size_limit(size_limit);
    let body = stream::once(async move { Ok::<Bytes, io::Error>(body) });
    let mut multipart = Multipart::with_constraints(body, boundary, constraints);
    let mut fields = Map::new();
    let mut images = Vec::new();
    let mut mask = None;

    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        let name = field.name().unwrap_or_default().to_string();
        let file_name = field.file_name().map(str::to_string);
        let content_type = field.content_type().map(ToString::to_string);
        let bytes = field.bytes().await.map_err(multipart_error)?;
        if matches!(name.as_str(), "image" | "image[]" | "mask") && file_name.is_some() {
            if bytes.is_empty() {
                return Err(api_error(
                    StatusCode::BAD_REQUEST,
                    "uploaded image must not be empty",
                    "invalid_request",
                ));
            }
            let data_url = image_data_url(&bytes, content_type.as_deref());
            if name == "mask" {
                mask = Some(data_url);
            } else {
                images.push(data_url);
            }
            continue;
        }
        let value = String::from_utf8(bytes.to_vec()).map_err(|_| {
            api_error(
                StatusCode::BAD_REQUEST,
                "multipart text fields must be UTF-8",
                "invalid_request",
            )
        })?;
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        if matches!(name.as_str(), "image" | "image[]") {
            images.push(value.to_string());
        } else if name == "mask" {
            mask = Some(value.to_string());
        } else if matches!(name.as_str(), "stream") {
            let parsed = match value.to_ascii_lowercase().as_str() {
                "1" | "true" | "yes" | "on" => true,
                "0" | "false" | "no" | "off" => false,
                _ => {
                    return Err(api_error(
                        StatusCode::BAD_REQUEST,
                        "stream must be a boolean",
                        "invalid_request",
                    ))
                }
            };
            fields.insert(name, Value::Bool(parsed));
        } else if matches!(name.as_str(), "n" | "output_compression" | "partial_images") {
            let parsed = value.parse::<u64>().map_err(|_| {
                api_error(
                    StatusCode::BAD_REQUEST,
                    "numeric multipart fields must be positive integers",
                    "invalid_request",
                )
            })?;
            fields.insert(name, Value::Number(parsed.into()));
        } else {
            fields.insert(name, Value::String(value.to_string()));
        }
    }
    Ok((fields, images, mask))
}

fn multipart_error(error: multer::Error) -> Response<Body> {
    let too_large = matches!(
        error,
        multer::Error::FieldSizeExceeded { .. } | multer::Error::StreamSizeExceeded { .. }
    );
    api_error(
        if too_large {
            StatusCode::PAYLOAD_TOO_LARGE
        } else {
            StatusCode::BAD_REQUEST
        },
        if too_large {
            "multipart image upload is too large"
        } else {
            "multipart image upload is invalid"
        },
        if too_large {
            "request_too_large"
        } else {
            "invalid_request"
        },
    )
}

fn image_data_url(bytes: &[u8], content_type: Option<&str>) -> String {
    let content_type = content_type
        .filter(|value| !value.trim().is_empty() && *value != "application/octet-stream")
        .unwrap_or_else(|| detect_image_content_type(bytes));
    format!("data:{content_type};base64,{}", STANDARD.encode(bytes))
}

fn detect_image_content_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        "image/gif"
    } else {
        "application/octet-stream"
    }
}

fn push_non_empty(target: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        target.push(value.to_string());
    }
}

async fn execute_prepared(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    prepared: PreparedImageRequest,
    endpoint: ImageEndpoint,
) -> Response<Body> {
    let request_id = request_id();
    let mut tried = HashSet::new();
    let mut attempt = 0_u16;
    let mut last_failure = None;

    while usize::from(attempt) < runtime.max_retry_candidates() {
        let Some((selected, lease)) = runtime.select_and_reserve_image(
            &key,
            &prepared.resolved_model,
            IMAGE_PROTOCOLS,
            &tried,
            now_ms(),
        ) else {
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let Some(mut route) = runtime.image_executor_route(&selected.candidate_id) else {
            continue;
        };
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        let account_route = route.account_id.is_some();
        let upstream_url = if account_route {
            Some(route.upstream_url.clone())
        } else {
            image_endpoint_url(route.upstream_url.clone(), endpoint)
        };
        let Some(upstream_url) = upstream_url else {
            last_failure = Some(AttemptFailure::invalid_request());
            continue;
        };
        let request_body = if account_route {
            match serde_json::to_vec(&build_account_request(
                &prepared,
                endpoint,
                &route.source_model,
            )) {
                Ok(body) => body,
                Err(_) => {
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        "image request could not be serialized",
                        "invalid_request",
                    )
                }
            }
        } else {
            direct_request_body(&prepared)
        };

        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let authorization = match runtime
            .prepare_authorization(&route.candidate_id, now_ms())
            .await
        {
            Ok(authorization) => authorization,
            Err(error) => {
                let failure = AttemptFailure::prepare(error);
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    failure.cooldown_ms,
                    route.half_open_probe,
                );
                let mut event = image_usage_event(
                    &request_id,
                    attempt,
                    &key,
                    &route,
                    &prepared,
                    false,
                    failure.status,
                    Some(failure.category.to_string()),
                    started,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        let mut upstream = runtime
            .request_client(&route.candidate_id, account_route || prepared.stream)
            .post(upstream_url)
            .header(AUTHORIZATION, authorization.authorization)
            .header(
                CONTENT_TYPE,
                if account_route {
                    HeaderValue::from_static("application/json")
                } else {
                    prepared.content_type.clone()
                },
            )
            .header(
                ACCEPT,
                if account_route || prepared.stream {
                    "text/event-stream"
                } else {
                    "application/json"
                },
            )
            .body(request_body);
        if let Some(account_id) = authorization.chatgpt_account_id {
            upstream = upstream.header("ChatGPT-Account-Id", account_id);
        }
        if let Some(originator) = authorization.originator {
            upstream = upstream.header("originator", originator);
        }
        let upstream = match upstream.send().await {
            Ok(upstream) => upstream,
            Err(_) => {
                let failure = AttemptFailure::transport();
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    TRANSIENT_COOLDOWN_MS,
                    route.half_open_probe,
                );
                let mut event = image_usage_event(
                    &request_id,
                    attempt,
                    &key,
                    &route,
                    &prepared,
                    false,
                    failure.status,
                    Some(failure.category.to_string()),
                    started,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };

        let status = upstream.status();
        let response_headers = upstream.headers().clone();
        let bytes =
            match crate::runtime::collect_limited(upstream, MAX_IMAGE_RESPONSE_BODY_BYTES).await {
                Ok(bytes) => bytes,
                Err(_) => {
                    let failure = AttemptFailure::body();
                    let state = apply_cooldown(
                        &runtime,
                        &route.candidate_id,
                        "*",
                        TRANSIENT_COOLDOWN_MS,
                        route.half_open_probe,
                    );
                    let mut event = image_usage_event(
                        &request_id,
                        attempt,
                        &key,
                        &route,
                        &prepared,
                        false,
                        failure.status,
                        Some(failure.category.to_string()),
                        started,
                    );
                    apply_failure_state(&mut event, state);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    continue;
                }
            };
        if !status.is_success() {
            let capability_failure = image_capability_unavailable(&bytes);
            if retryable_status(status, false) || capability_failure {
                let state = if capability_failure {
                    apply_cooldown(
                        &runtime,
                        &route.candidate_id,
                        IMAGE_API_MODEL,
                        TRANSIENT_COOLDOWN_MS,
                        route.half_open_probe,
                    )
                } else {
                    apply_status_cooldown(
                        &runtime,
                        &route.candidate_id,
                        IMAGE_API_MODEL,
                        status,
                        &response_headers,
                        route.half_open_probe,
                    )
                };
                let mut event = image_usage_event(
                    &request_id,
                    attempt,
                    &key,
                    &route,
                    &prepared,
                    false,
                    status,
                    Some(if capability_failure {
                        "image_generation_not_enabled".to_string()
                    } else {
                        "upstream_status".to_string()
                    }),
                    started,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(AttemptFailure::status(status));
                continue;
            }
            let mut event = image_usage_event(
                &request_id,
                attempt,
                &key,
                &route,
                &prepared,
                false,
                status,
                Some("upstream_status".to_string()),
                started,
            );
            populate_tokens(&mut event, &bytes);
            emit_usage(&runtime, event);
            return proxy_response(status, &response_headers, Body::from(bytes));
        }

        if !account_route {
            let mut event = image_usage_event(
                &request_id,
                attempt,
                &key,
                &route,
                &prepared,
                true,
                status,
                None,
                started,
            );
            event.consecutive_failures = Some(0);
            populate_tokens(&mut event, &bytes);
            runtime.record_success_with_metrics(
                &route.candidate_id,
                IMAGE_API_MODEL,
                now_ms(),
                None,
                event.latency_ms,
            );
            emit_usage(&runtime, event);
            drop(lease);
            return if prepared.stream {
                proxy_sse_response(status, &response_headers, Body::from(bytes))
            } else {
                proxy_response(status, &response_headers, Body::from(bytes))
            };
        }

        let translated = match translate_account_response(
            &bytes,
            &prepared.response_format,
            endpoint.stream_prefix(),
        ) {
            Ok(translated) => translated,
            Err(failure) if failure.retryable => {
                let state = if failure.category == "image_generation_not_enabled" {
                    apply_cooldown(
                        &runtime,
                        &route.candidate_id,
                        IMAGE_API_MODEL,
                        TRANSIENT_COOLDOWN_MS,
                        route.half_open_probe,
                    )
                } else {
                    apply_status_cooldown(
                        &runtime,
                        &route.candidate_id,
                        IMAGE_API_MODEL,
                        failure.status,
                        &response_headers,
                        route.half_open_probe,
                    )
                };
                let mut event = image_usage_event(
                    &request_id,
                    attempt,
                    &key,
                    &route,
                    &prepared,
                    false,
                    failure.status,
                    Some(failure.category.to_string()),
                    started,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(AttemptFailure::status(failure.status));
                continue;
            }
            Err(failure) => {
                let event = image_usage_event(
                    &request_id,
                    attempt,
                    &key,
                    &route,
                    &prepared,
                    false,
                    failure.status,
                    Some(failure.category.to_string()),
                    started,
                );
                emit_usage(&runtime, event);
                return image_error_response(failure);
            }
        };
        let mut event = image_usage_event(
            &request_id,
            attempt,
            &key,
            &route,
            &prepared,
            true,
            status,
            None,
            started,
        );
        event.consecutive_failures = Some(0);
        if let Some(usage) = translated.usage.as_ref() {
            apply_usage(&mut event, usage);
        }
        runtime.record_success_with_metrics(
            &route.candidate_id,
            IMAGE_API_MODEL,
            now_ms(),
            None,
            event.latency_ms,
        );
        emit_usage(&runtime, event);
        drop(lease);
        return if prepared.stream {
            proxy_sse_response(status, &response_headers, Body::from(translated.stream))
        } else {
            proxy_json_response(status, &response_headers, Body::from(translated.json))
        };
    }

    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(retry_at) = runtime.earliest_retry_at(
            &key,
            &prepared.resolved_model,
            IMAGE_PROTOCOLS,
            &HashSet::new(),
            now_ms(),
        ) {
            return cooldown_error(retry_at);
        }
    }
    api_error(failure.status, failure.message, failure.category)
}

#[allow(clippy::too_many_arguments)]
fn image_usage_event(
    request_id: &str,
    attempt: u16,
    key: &AuthenticatedKey,
    route: &ExecutorRoute,
    request: &PreparedImageRequest,
    success: bool,
    status: StatusCode,
    category: Option<String>,
    started: Instant,
) -> UsageEvent {
    usage_event(
        request_id,
        attempt,
        &key.id,
        route,
        &request.requested_model,
        success,
        status.as_u16(),
        category,
        started.elapsed().as_millis() as u64,
    )
}

fn direct_request_body(request: &PreparedImageRequest) -> Vec<u8> {
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

fn build_account_request(
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
        Value::String(IMAGE_API_MODEL.to_string()),
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

fn image_endpoint_url(mut responses_url: url::Url, endpoint: ImageEndpoint) -> Option<url::Url> {
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

fn translate_account_response(
    bytes: &[u8],
    response_format: &str,
    stream_prefix: &str,
) -> Result<TranslatedImageResponse, ImageFailure> {
    let mut output = Vec::new();
    let mut partials = Vec::new();
    let mut completed = None;

    if let Ok(value) = serde_json::from_slice::<Value>(bytes) {
        if let Some(failure) = image_failure_from_event(&value) {
            return Err(failure);
        }
        completed = Some(value.get("response").cloned().unwrap_or(value));
    } else {
        let mut offset = 0;
        while let Some(end) = sse_event_end(&bytes[offset..]) {
            let event = sse_json(&bytes[offset..offset + end]);
            offset += end;
            let Some(event) = event else {
                continue;
            };
            if let Some(failure) = image_failure_from_event(&event) {
                return Err(failure);
            }
            match event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
            {
                "response.image_generation_call.partial_image" => partials.push(event),
                "response.output_item.done" => {
                    if let Some(item) = event.get("item") {
                        output.push(item.clone());
                    }
                }
                "response.completed" | "response.done" => {
                    completed = Some(event.get("response").cloned().unwrap_or(event));
                    break;
                }
                _ => {}
            }
        }
    }

    let Some(mut completed) = completed else {
        return Err(ImageFailure {
            status: StatusCode::BAD_GATEWAY,
            category: "stream_incomplete",
            code: "stream_incomplete".to_string(),
            message: "upstream image stream ended before completion".to_string(),
            retryable: true,
        });
    };
    if completed
        .get("output")
        .and_then(Value::as_array)
        .is_none_or(Vec::is_empty)
        && !output.is_empty()
    {
        completed["output"] = Value::Array(output);
    }
    let images = completed
        .get("output")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(image_result)
        .collect::<Vec<_>>();
    if images.is_empty() {
        return Err(ImageFailure {
            status: StatusCode::BAD_GATEWAY,
            category: "image_output_missing",
            code: "image_output_missing".to_string(),
            message: "upstream did not return image output".to_string(),
            retryable: true,
        });
    }
    let created = completed
        .get("created_at")
        .or_else(|| completed.get("created"))
        .and_then(Value::as_i64)
        .unwrap_or_else(|| (now_ms() / 1_000).min(i64::MAX as u64) as i64);
    let usage = completed
        .pointer("/tool_usage/image_gen")
        .or_else(|| completed.get("usage"))
        .cloned();
    let data = images
        .iter()
        .map(|image| image_api_item(image, response_format))
        .collect::<Vec<_>>();
    let mut json_body = json!({"created": created, "data": data});
    if let Some(usage) = usage.clone() {
        json_body["usage"] = usage;
    }
    if let Some(first) = images.first() {
        for name in ["background", "output_format", "quality", "size"] {
            if let Some(value) = first.get(name).filter(|value| !value.is_null()) {
                json_body[name] = value.clone();
            }
        }
    }

    let mut stream_body = Vec::new();
    for partial in partials {
        let Some(result) = partial
            .get("partial_image_b64")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let output_format = partial
            .get("output_format")
            .and_then(Value::as_str)
            .unwrap_or("png");
        let event_name = format!("{stream_prefix}.partial_image");
        let mut data = json!({
            "type": event_name,
            "partial_image_index": partial.get("partial_image_index").and_then(Value::as_u64).unwrap_or(0),
        });
        insert_image_payload(&mut data, result, output_format, response_format);
        push_sse(&mut stream_body, &event_name, &data);
    }
    let event_name = format!("{stream_prefix}.completed");
    for image in &images {
        let mut data = image_api_item(image, response_format);
        data["type"] = Value::String(event_name.clone());
        if let Some(usage) = usage.clone() {
            data["usage"] = usage;
        }
        push_sse(&mut stream_body, &event_name, &data);
    }
    stream_body.extend_from_slice(b"data: [DONE]\n\n");

    Ok(TranslatedImageResponse {
        json: serde_json::to_vec(&json_body).unwrap_or_else(|_| b"{}".to_vec()),
        stream: stream_body,
        usage,
    })
}

fn sse_json(event: &[u8]) -> Option<Value> {
    let mut data = Vec::new();
    for line in event.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Some(value) = line.strip_prefix(b"data:") else {
            continue;
        };
        if !data.is_empty() {
            data.push(b'\n');
        }
        data.extend_from_slice(value.strip_prefix(b" ").unwrap_or(value));
    }
    (!data.is_empty() && data != b"[DONE]")
        .then(|| serde_json::from_slice(&data).ok())
        .flatten()
}

fn image_result(item: &Value) -> Option<Value> {
    (item.get("type").and_then(Value::as_str) == Some("image_generation_call"))
        .then(|| item.get("result").and_then(Value::as_str))
        .flatten()
        .filter(|result| !result.trim().is_empty())
        .map(|_| item.clone())
}

fn image_api_item(image: &Value, response_format: &str) -> Value {
    let result = image
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let output_format = image
        .get("output_format")
        .and_then(Value::as_str)
        .unwrap_or("png");
    let mut item = Value::Object(Map::new());
    insert_image_payload(&mut item, result, output_format, response_format);
    if let Some(prompt) = image
        .get("revised_prompt")
        .and_then(Value::as_str)
        .filter(|prompt| !prompt.is_empty())
    {
        item["revised_prompt"] = Value::String(prompt.to_string());
    }
    item
}

fn insert_image_payload(target: &mut Value, result: &str, output_format: &str, format: &str) {
    if format.eq_ignore_ascii_case("url") {
        target["url"] = Value::String(format!(
            "data:{};base64,{result}",
            image_mime_type(output_format)
        ));
    } else {
        target["b64_json"] = Value::String(result.to_string());
    }
}

fn image_mime_type(output_format: &str) -> &'static str {
    match output_format.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "image/png",
    }
}

fn push_sse(target: &mut Vec<u8>, event_name: &str, data: &Value) {
    target.extend_from_slice(b"event: ");
    target.extend_from_slice(event_name.as_bytes());
    target.extend_from_slice(b"\ndata: ");
    target.extend_from_slice(data.to_string().as_bytes());
    target.extend_from_slice(b"\n\n");
}

fn image_failure_from_event(value: &Value) -> Option<ImageFailure> {
    let event_type = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let response = value.get("response").unwrap_or(value);
    let error = value
        .get("error")
        .or_else(|| response.get("error"))
        .filter(|error| !error.is_null());
    let incomplete_reason = response
        .pointer("/incomplete_details/reason")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if error.is_none()
        && !matches!(
            event_type,
            "response.failed" | "response.incomplete" | "error"
        )
    {
        return None;
    }
    let error = error.unwrap_or(&Value::Null);
    let error_type = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .unwrap_or(if event_type == "response.incomplete" {
            "response_incomplete"
        } else {
            "upstream_error"
        });
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if incomplete_reason.is_empty() {
                "upstream image generation failed".to_string()
            } else {
                format!("upstream image generation incomplete: {incomplete_reason}")
            }
        });
    let normalized =
        format!("{error_type} {code} {message} {incomplete_reason}").to_ascii_lowercase();
    let capability = normalized.contains("image generation is not enabled")
        || normalized.contains("image_generation_not_enabled");
    let user_error = error_type.eq_ignore_ascii_case("image_generation_user_error")
        || normalized.contains("moderation")
        || normalized.contains("content_policy")
        || normalized.contains("content filter")
        || normalized.contains("policy_violation")
        || normalized.contains("safety_violation");
    let (status, category, retryable) = if capability {
        (
            StatusCode::BAD_GATEWAY,
            "image_generation_not_enabled",
            true,
        )
    } else if user_error {
        (
            StatusCode::BAD_REQUEST,
            "image_generation_user_error",
            false,
        )
    } else if normalized.contains("rate_limit") {
        (StatusCode::TOO_MANY_REQUESTS, "rate_limit", true)
    } else if normalized.contains("authentication") || normalized.contains("unauthorized") {
        (StatusCode::UNAUTHORIZED, "authentication", true)
    } else if normalized.contains("permission") || normalized.contains("forbidden") {
        (StatusCode::FORBIDDEN, "permission", true)
    } else {
        (StatusCode::BAD_GATEWAY, "upstream_image_error", true)
    };
    Some(ImageFailure {
        status,
        category,
        code: code.to_string(),
        message,
        retryable,
    })
}

fn image_capability_unavailable(bytes: &[u8]) -> bool {
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    text.contains("image generation is not enabled")
        || text.contains("image_generation_not_enabled")
}

fn image_error_response(failure: ImageFailure) -> Response<Body> {
    (
        failure.status,
        Json(json!({
            "error": {
                "message": failure.message,
                "type": failure.category,
                "code": failure.code,
            }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        };
        let body = build_account_request(&request, ImageEndpoint::Generations, "gpt-5.4-mini");
        assert_eq!(body["model"], "gpt-5.4-mini");
        assert_eq!(body["tools"][0]["model"], IMAGE_API_MODEL);
        assert_eq!(body["tools"][0]["quality"], "low");
        assert!(body["tools"][0].get("size").is_none());
    }

    #[test]
    fn completed_response_becomes_images_api_payload() {
        let translated = translate_account_response(
            b"data: {\"type\":\"response.completed\",\"response\":{\"created_at\":7,\"output\":[{\"type\":\"image_generation_call\",\"result\":\"aW1hZ2U=\",\"output_format\":\"png\"}],\"usage\":{\"input_tokens\":3,\"output_tokens\":2}}}\n\n",
            "b64_json",
            "image_generation",
        )
        .unwrap();
        let body: Value = serde_json::from_slice(&translated.json).unwrap();
        assert_eq!(body["created"], 7);
        assert_eq!(body["data"][0]["b64_json"], "aW1hZ2U=");
        assert!(String::from_utf8(translated.stream)
            .unwrap()
            .contains("image_generation.completed"));
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
