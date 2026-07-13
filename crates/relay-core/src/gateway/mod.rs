use crate::runtime::{AuthenticatedKey, ExecutorPrepareError, ExecutorRoute};
use crate::{Error, GatewayRuntime, UsageEvent, WireApi};
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{
    AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, HOST, RETRY_AFTER, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{stream, Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const MAX_SSE_EVENT_BYTES: usize = 16 * 1024 * 1024;
const TRANSIENT_COOLDOWN_MS: u64 = 60_000;
const MAX_RATE_LIMIT_COOLDOWN_MS: u64 = 30 * 60_000;
type UpstreamStream = Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>;
type CompletionCallback = Arc<dyn Fn(&mut UsageEvent) + Send + Sync>;

pub fn router(runtime: Arc<GatewayRuntime>) -> Router {
    Router::new()
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(runtime)
}

async fn models(State(runtime): State<Arc<GatewayRuntime>>, headers: HeaderMap) -> Response<Body> {
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    let models = runtime.visible_models(
        &key,
        &[WireApi::Responses, WireApi::ChatCompletions],
        now_ms(),
    );
    Json(json!({
        "object": "list",
        "data": models.into_iter().map(|id| json!({
            "id": id,
            "object": "model",
            "owned_by": "zenith-relay",
        })).collect::<Vec<_>>()
    }))
    .into_response()
}

async fn responses(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    execute_client_request(runtime, headers, body, WireApi::Responses).await
}

async fn chat_completions(
    State(runtime): State<Arc<GatewayRuntime>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    execute_client_request(runtime, headers, body, WireApi::ChatCompletions).await
}

async fn execute_client_request(
    runtime: Arc<GatewayRuntime>,
    headers: HeaderMap,
    body: Bytes,
    wire_api: WireApi,
) -> Response<Body> {
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };

    let request: Value = match serde_json::from_slice(&body) {
        Ok(Value::Object(request)) => Value::Object(request),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "request body must be a JSON object",
                "invalid_request",
            )
        }
    };
    let Some(requested_model) = request
        .get("model")
        .and_then(Value::as_str)
        .filter(|model| !model.trim().is_empty())
        .map(str::to_string)
    else {
        return api_error(
            StatusCode::BAD_REQUEST,
            "model must be a non-empty string",
            "invalid_request",
        );
    };
    let stream = match request.get("stream") {
        Some(Value::Bool(stream)) => *stream,
        Some(_) => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "stream must be a boolean",
                "invalid_request",
            )
        }
        None => false,
    };
    let visible_models = runtime.visible_models(&key, candidate_protocols(wire_api), now_ms());
    if !visible_models
        .iter()
        .any(|model| model.eq_ignore_ascii_case(&requested_model))
    {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    }
    let Some(resolved_model) = runtime.resolve_model(&key, &requested_model) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    };
    let session = affinity_session(&headers, &request);
    let affinity_key = runtime.affinity_key(&key.id, wire_api, &resolved_model, session.as_deref());
    execute_request(
        runtime,
        key,
        request,
        requested_model,
        resolved_model,
        stream,
        request_id(),
        affinity_key,
        wire_api,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn execute_request(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    request: Value,
    requested_model: String,
    resolved_model: String,
    stream: bool,
    request_id: String,
    affinity_key: Option<String>,
    wire_api: WireApi,
) -> Response<Body> {
    let mut tried = HashSet::new();
    let mut attempt = 0_u16;
    let mut last_failure = None;
    let has_previous_response_id = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());

    while usize::from(attempt) < runtime.max_retry_candidates() {
        let selected = runtime.select_and_reserve(
            &key,
            &resolved_model,
            candidate_protocols(wire_api),
            &tried,
            affinity_key.as_deref(),
            now_ms(),
        );
        let Some((selected, lease)) = selected else {
            if attempt == 0 {
                if let Some(retry_at) = runtime.earliest_retry_at(
                    &key,
                    &resolved_model,
                    candidate_protocols(wire_api),
                    &tried,
                    now_ms(),
                ) {
                    return cooldown_error(retry_at);
                }
            }
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let Some(route) = runtime.executor_route(&selected.candidate_id, &resolved_model) else {
            continue;
        };
        let source_model = route.source_model.clone();
        let responses_via_chat =
            wire_api == WireApi::Responses && route.wire_api == WireApi::ChatCompletions;
        let chat_via_responses =
            wire_api == WireApi::ChatCompletions && route.wire_api == WireApi::Responses;
        let account_route = route.account_id.is_some();
        let request_body = if responses_via_chat {
            match translate_responses_request(&request, &source_model, false) {
                Ok(body) => body,
                Err(failure) => {
                    last_failure = Some(failure);
                    continue;
                }
            }
        } else if chat_via_responses {
            match translate_chat_request(&request, &source_model, false) {
                Ok(body) if account_route => match normalize_account_request_body(&body) {
                    Ok(body) => body,
                    Err(failure) => {
                        last_failure = Some(failure);
                        continue;
                    }
                },
                Ok(body) => body,
                Err(failure) => {
                    last_failure = Some(failure);
                    continue;
                }
            }
        } else {
            let mut upstream_request = request.clone();
            let Value::Object(object) = &mut upstream_request else {
                unreachable!("request object was validated before execution")
            };
            object.insert("model".to_string(), Value::String(source_model.clone()));
            if account_route {
                normalize_account_request(object);
            }
            match serde_json::to_vec(&upstream_request) {
                Ok(body) => body,
                Err(_) => {
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        "request body could not be serialized",
                        "invalid_request",
                    )
                }
            }
        };

        // ponytail: cross-protocol streams are buffered into one terminal SSE sequence; add delta translation when adapter TTFT matters.
        let upstream_stream = stream && !responses_via_chat && !chat_via_responses;
        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let prepared = match runtime
            .prepare_authorization(&route.candidate_id, now_ms())
            .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let failure = AttemptFailure::prepare(error);
                let state = apply_cooldown(&runtime, &route.candidate_id, "*", failure.cooldown_ms);
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        let client = runtime.request_client(&route.candidate_id, upstream_stream);
        let mut upstream_request = client
            .post(route.upstream_url.clone())
            .header(AUTHORIZATION, prepared.authorization)
            .header(CONTENT_TYPE, "application/json");
        if let Some(account_id) = prepared.chatgpt_account_id {
            upstream_request = upstream_request.header("ChatGPT-Account-Id", account_id);
        }
        if let Some(originator) = prepared.originator {
            upstream_request = upstream_request.header("originator", originator);
        }
        let upstream = upstream_request.body(request_body).send().await;
        let upstream = match upstream {
            Ok(upstream) => upstream,
            Err(_) => {
                let failure = AttemptFailure::transport();
                let state =
                    apply_cooldown(&runtime, &route.candidate_id, "*", TRANSIENT_COOLDOWN_MS);
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };

        let status = upstream.status();
        let response_headers = upstream.headers().clone();
        if !status.is_success() {
            if retryable_status(status, has_previous_response_id) {
                let _ = crate::runtime::collect_limited(
                    upstream,
                    crate::runtime::MAX_NON_STREAM_BODY_BYTES,
                )
                .await;
                let state = apply_status_cooldown(
                    &runtime,
                    &route.candidate_id,
                    &source_model,
                    status,
                    &response_headers,
                );
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    status.as_u16(),
                    Some("upstream_status".to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(AttemptFailure::status(status));
                continue;
            }
            return finish_non_stream(
                &runtime,
                upstream,
                &response_headers,
                usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    status.as_u16(),
                    Some("upstream_status".to_string()),
                    0,
                ),
                started,
            )
            .await;
        }

        if !upstream_stream {
            let bytes = match crate::runtime::collect_limited(
                upstream,
                crate::runtime::MAX_NON_STREAM_BODY_BYTES,
            )
            .await
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    let too_large = matches!(error, Error::UpstreamBodyTooLarge);
                    let state =
                        apply_cooldown(&runtime, &route.candidate_id, "*", TRANSIENT_COOLDOWN_MS);
                    let mut event = usage_event(
                        &request_id,
                        attempt,
                        &key.id,
                        &route,
                        &requested_model,
                        false,
                        StatusCode::BAD_GATEWAY.as_u16(),
                        Some(if too_large {
                            "upstream_body_too_large".to_string()
                        } else {
                            "upstream_body".to_string()
                        }),
                        started.elapsed().as_millis() as u64,
                    );
                    apply_failure_state(&mut event, state);
                    emit_usage(&runtime, event);
                    last_failure = Some(AttemptFailure::body());
                    continue;
                }
            };
            let bytes = if account_route {
                match completed_account_response(&bytes) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let state = apply_cooldown(
                            &runtime,
                            &route.candidate_id,
                            &source_model,
                            TRANSIENT_COOLDOWN_MS,
                        );
                        let mut event = usage_event(
                            &request_id,
                            attempt,
                            &key.id,
                            &route,
                            &requested_model,
                            false,
                            failure.status.as_u16(),
                            Some(failure.category.to_string()),
                            started.elapsed().as_millis() as u64,
                        );
                        apply_failure_state(&mut event, state);
                        emit_usage(&runtime, event);
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else {
                bytes
            };
            let bytes = if responses_via_chat {
                match translate_chat_response(&bytes) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let state = apply_cooldown(
                            &runtime,
                            &route.candidate_id,
                            &source_model,
                            TRANSIENT_COOLDOWN_MS,
                        );
                        let mut event = usage_event(
                            &request_id,
                            attempt,
                            &key.id,
                            &route,
                            &requested_model,
                            false,
                            failure.status.as_u16(),
                            Some(failure.category.to_string()),
                            started.elapsed().as_millis() as u64,
                        );
                        apply_failure_state(&mut event, state);
                        emit_usage(&runtime, event);
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else if chat_via_responses {
                match translate_responses_response(&bytes) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let state = apply_cooldown(
                            &runtime,
                            &route.candidate_id,
                            &source_model,
                            TRANSIENT_COOLDOWN_MS,
                        );
                        let mut event = usage_event(
                            &request_id,
                            attempt,
                            &key.id,
                            &route,
                            &requested_model,
                            false,
                            failure.status.as_u16(),
                            Some(failure.category.to_string()),
                            started.elapsed().as_millis() as u64,
                        );
                        apply_failure_state(&mut event, state);
                        emit_usage(&runtime, event);
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else {
                bytes
            };
            runtime.record_success(&route.candidate_id, &source_model, now_ms());
            let mut event = usage_event(
                &request_id,
                attempt,
                &key.id,
                &route,
                &requested_model,
                true,
                status.as_u16(),
                None,
                started.elapsed().as_millis() as u64,
            );
            event.consecutive_failures = Some(0);
            populate_tokens(&mut event, &bytes);
            emit_usage(&runtime, event);
            runtime.bind_affinity(affinity_key.as_deref(), &route.candidate_id, now_ms());
            if stream {
                let body = match wire_api {
                    WireApi::Responses => completed_sse(&bytes),
                    WireApi::ChatCompletions => completed_chat_sse(&bytes),
                    WireApi::Messages => Bytes::new(),
                };
                return proxy_sse_response(status, &response_headers, Body::from(body));
            }
            if account_route {
                return proxy_json_response(status, &response_headers, Body::from(bytes));
            }
            return proxy_response(status, &response_headers, Body::from(bytes));
        }

        match bootstrap_stream(upstream).await {
            Ok((headers, first, remaining)) => {
                let completion_runtime = runtime.clone();
                let completion_source = route.candidate_id.clone();
                let completion_model = source_model.clone();
                let completion_affinity = affinity_key.clone();
                let completion: CompletionCallback = Arc::new(move |event| {
                    lease.release();
                    if event.success {
                        completion_runtime.record_success(
                            &completion_source,
                            &completion_model,
                            now_ms(),
                        );
                        event.consecutive_failures = Some(0);
                        completion_runtime.bind_affinity(
                            completion_affinity.as_deref(),
                            &completion_source,
                            now_ms(),
                        );
                    } else if event.error_category.as_deref() != Some("client_cancelled") {
                        let state = apply_cooldown(
                            &completion_runtime,
                            &completion_source,
                            &completion_model,
                            TRANSIENT_COOLDOWN_MS,
                        );
                        apply_failure_state(event, state);
                    }
                });
                let combined =
                    stream::once(async move { Ok::<_, reqwest::Error>(first) }).chain(remaining);
                let usage_stream = UsageStream::new(
                    combined,
                    runtime.usage.clone(),
                    usage_event(
                        &request_id,
                        attempt,
                        &key.id,
                        &route,
                        &requested_model,
                        true,
                        status.as_u16(),
                        None,
                        0,
                    ),
                    started,
                    completion,
                );
                return proxy_sse_response(status, &headers, Body::from_stream(usage_stream));
            }
            Err(failure) => {
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    &source_model,
                    TRANSIENT_COOLDOWN_MS,
                );
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
            }
        }
    }

    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(retry_at) = runtime.earliest_retry_at(
            &key,
            &resolved_model,
            candidate_protocols(wire_api),
            &HashSet::new(),
            now_ms(),
        ) {
            return cooldown_error(retry_at);
        }
    }
    api_error(failure.status, failure.message, failure.category)
}

async fn finish_non_stream(
    runtime: &GatewayRuntime,
    upstream: reqwest::Response,
    response_headers: &reqwest::header::HeaderMap,
    mut event: UsageEvent,
    started: Instant,
) -> Response<Body> {
    let status = upstream.status();
    match crate::runtime::collect_limited(upstream, crate::runtime::MAX_NON_STREAM_BODY_BYTES).await
    {
        Ok(bytes) => {
            event.latency_ms = started.elapsed().as_millis() as u64;
            populate_tokens(&mut event, &bytes);
            emit_usage(runtime, event);
            proxy_response(status, response_headers, Body::from(bytes))
        }
        Err(error) => {
            event.success = false;
            event.http_status = StatusCode::BAD_GATEWAY.as_u16();
            let too_large = matches!(error, Error::UpstreamBodyTooLarge);
            event.error_category = Some(if too_large {
                "upstream_body_too_large".to_string()
            } else {
                "upstream_body".to_string()
            });
            event.latency_ms = started.elapsed().as_millis() as u64;
            emit_usage(runtime, event);
            api_error(
                StatusCode::BAD_GATEWAY,
                if too_large {
                    "upstream response is too large"
                } else {
                    "upstream response failed"
                },
                "upstream_error",
            )
        }
    }
}

async fn bootstrap_stream(
    upstream: reqwest::Response,
) -> Result<(reqwest::header::HeaderMap, Bytes, UpstreamStream), AttemptFailure> {
    let headers = upstream.headers().clone();
    let mut stream: UpstreamStream = Box::pin(upstream.bytes_stream());
    let mut buffer = Vec::new();
    loop {
        match stream.next().await {
            Some(Ok(chunk)) => {
                if buffer.len().saturating_add(chunk.len()) > MAX_SSE_EVENT_BYTES {
                    return Err(AttemptFailure::stream("stream_event_too_large"));
                }
                buffer.extend_from_slice(&chunk);
                let mut inspected = 0;
                while let Some(end) = sse_event_end(&buffer[inspected..]) {
                    let absolute_end = inspected + end;
                    let event = parse_sse_event(&buffer[inspected..absolute_end]);
                    if event.has_data && !event.valid {
                        return Err(AttemptFailure::stream("stream_invalid"));
                    }
                    if event.outcome == Some(TerminalOutcome::Failure) {
                        return Err(AttemptFailure::stream("upstream_terminal"));
                    }
                    if event.has_data {
                        return Ok((headers, Bytes::from(buffer), stream));
                    }
                    inspected = absolute_end;
                }
            }
            Some(Err(_)) => return Err(AttemptFailure::stream("upstream_stream")),
            None => return Err(AttemptFailure::stream("stream_incomplete")),
        }
    }
}

fn translate_responses_request(
    request: &Value,
    model: &str,
    stream: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let object = request
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    if object
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Err(AttemptFailure::invalid_request());
    }

    let mut messages = Vec::new();
    if let Some(instructions) = object.get("instructions") {
        let Some(instructions) = instructions.as_str() else {
            return Err(AttemptFailure::invalid_request());
        };
        if !instructions.trim().is_empty() {
            messages.push(json!({"role": "system", "content": instructions}));
        }
    }
    let input = object
        .get("input")
        .ok_or_else(AttemptFailure::invalid_request)?;
    match input {
        Value::String(text) => messages.push(json!({"role": "user", "content": text})),
        Value::Array(items)
            if !items.is_empty()
                && items
                    .iter()
                    .all(|item| item.get("role").and_then(Value::as_str).is_some()) =>
        {
            for item in items {
                let role = item
                    .get("role")
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                if !matches!(role, "developer" | "system" | "user" | "assistant" | "tool") {
                    return Err(AttemptFailure::invalid_request());
                }
                let content = translate_message_content(
                    item.get("content")
                        .ok_or_else(AttemptFailure::invalid_request)?,
                )?;
                messages.push(json!({"role": role, "content": content}));
            }
        }
        Value::Array(items) if !items.is_empty() => messages.push(json!({
            "role": "user",
            "content": translate_message_content(input)?,
        })),
        _ => return Err(AttemptFailure::invalid_request()),
    }

    let mut translated = serde_json::Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("messages".to_string(), Value::Array(messages)),
        ("stream".to_string(), Value::Bool(stream)),
    ]);
    for field in ["temperature", "top_p", "stop", "parallel_tool_calls"] {
        if let Some(value) = object.get(field) {
            translated.insert(field.to_string(), value.clone());
        }
    }
    if let Some(value) = object.get("max_output_tokens") {
        translated.insert("max_completion_tokens".to_string(), value.clone());
    }
    if let Some(tools) = object.get("tools") {
        translated.insert("tools".to_string(), translate_tools(tools)?);
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        translated.insert(
            "tool_choice".to_string(),
            translate_tool_choice(tool_choice)?,
        );
    }
    serde_json::to_vec(&Value::Object(translated)).map_err(|_| AttemptFailure::invalid_request())
}

fn translate_chat_request(
    request: &Value,
    model: &str,
    stream: bool,
) -> Result<Vec<u8>, AttemptFailure> {
    let object = request
        .as_object()
        .ok_or_else(AttemptFailure::invalid_request)?;
    let messages = object
        .get("messages")
        .and_then(Value::as_array)
        .filter(|messages| !messages.is_empty())
        .ok_or_else(AttemptFailure::invalid_request)?;
    let mut input = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .ok_or_else(AttemptFailure::invalid_request)?;
        if role == "tool" {
            let call_id = message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let output = message
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            input.push(json!({
                "type": "function_call_output",
                "call_id": call_id,
                "output": output,
            }));
            continue;
        }
        if !matches!(role, "developer" | "system" | "user" | "assistant") {
            return Err(AttemptFailure::invalid_request());
        }
        if let Some(content) = message.get("content").filter(|content| !content.is_null()) {
            input.push(json!({
                "role": role,
                "content": translate_chat_message_content(content)?,
            }));
        }
        if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in tool_calls {
                let function = call
                    .get("function")
                    .and_then(Value::as_object)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                input.push(json!({
                    "type": "function_call",
                    "call_id": call.get("id").and_then(Value::as_str).ok_or_else(AttemptFailure::invalid_request)?,
                    "name": function.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::invalid_request)?,
                    "arguments": function.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                }));
            }
        }
    }
    if input.is_empty() {
        return Err(AttemptFailure::invalid_request());
    }

    let mut translated = serde_json::Map::from_iter([
        ("model".to_string(), Value::String(model.to_string())),
        ("input".to_string(), Value::Array(input)),
        ("stream".to_string(), Value::Bool(stream)),
    ]);
    for field in ["temperature", "top_p", "parallel_tool_calls"] {
        if let Some(value) = object.get(field) {
            translated.insert(field.to_string(), value.clone());
        }
    }
    if let Some(value) = object
        .get("max_completion_tokens")
        .or_else(|| object.get("max_tokens"))
    {
        translated.insert("max_output_tokens".to_string(), value.clone());
    }
    if let Some(tools) = object.get("tools") {
        translated.insert("tools".to_string(), translate_chat_tools(tools)?);
    }
    if let Some(tool_choice) = object.get("tool_choice") {
        translated.insert(
            "tool_choice".to_string(),
            translate_chat_tool_choice(tool_choice)?,
        );
    }
    serde_json::to_vec(&Value::Object(translated)).map_err(|_| AttemptFailure::invalid_request())
}

fn normalize_account_request_body(body: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let mut request =
        serde_json::from_slice::<Value>(body).map_err(|_| AttemptFailure::invalid_request())?;
    let object = request
        .as_object_mut()
        .ok_or_else(AttemptFailure::invalid_request)?;
    normalize_account_request(object);
    serde_json::to_vec(&request).map_err(|_| AttemptFailure::invalid_request())
}

fn normalize_account_request(object: &mut serde_json::Map<String, Value>) {
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    object.remove("max_output_tokens");
    if let Some(Value::String(text)) = object.get("input") {
        let text = text.clone();
        object.insert(
            "input".to_string(),
            json!([{"role": "user", "content": [{"type": "input_text", "text": text}]}]),
        );
    }
}

fn translate_chat_message_content(content: &Value) -> Result<Value, AttemptFailure> {
    match content {
        Value::String(_) => Ok(content.clone()),
        Value::Array(items) => items
            .iter()
            .map(|item| match item.get("type").and_then(Value::as_str) {
                Some("text") => item
                    .get("text")
                    .and_then(Value::as_str)
                    .map(|text| json!({"type": "input_text", "text": text}))
                    .ok_or_else(AttemptFailure::invalid_request),
                Some("image_url") => item
                    .get("image_url")
                    .and_then(|image| image.get("url"))
                    .and_then(Value::as_str)
                    .map(|url| json!({"type": "input_image", "image_url": url}))
                    .ok_or_else(AttemptFailure::invalid_request),
                _ => Err(AttemptFailure::invalid_request()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn translate_chat_tools(tools: &Value) -> Result<Value, AttemptFailure> {
    let tools = tools
        .as_array()
        .ok_or_else(AttemptFailure::invalid_request)?;
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AttemptFailure::invalid_request());
            }
            let function = tool
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let mut translated = serde_json::Map::from_iter([
                ("type".to_string(), Value::String("function".to_string())),
                (
                    "name".to_string(),
                    function
                        .get("name")
                        .cloned()
                        .ok_or_else(AttemptFailure::invalid_request)?,
                ),
            ]);
            for field in ["description", "parameters", "strict"] {
                if let Some(value) = function.get(field) {
                    translated.insert(field.to_string(), value.clone());
                }
            }
            Ok(Value::Object(translated))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn translate_chat_tool_choice(tool_choice: &Value) -> Result<Value, AttemptFailure> {
    if tool_choice.is_string() {
        return Ok(tool_choice.clone());
    }
    let name = tool_choice
        .get("function")
        .and_then(|function| function.get("name"))
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    Ok(json!({"type": "function", "name": name}))
}

fn translate_message_content(content: &Value) -> Result<Value, AttemptFailure> {
    match content {
        Value::String(_) => Ok(content.clone()),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if let Some(text) = item.as_str() {
                    return Ok(json!({"type": "text", "text": text}));
                }
                let kind = item
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::invalid_request)?;
                match kind {
                    "input_text" | "output_text" | "text" => item
                        .get("text")
                        .and_then(Value::as_str)
                        .map(|text| json!({"type": "text", "text": text}))
                        .ok_or_else(AttemptFailure::invalid_request),
                    "input_image" => item
                        .get("image_url")
                        .or_else(|| item.get("url"))
                        .and_then(Value::as_str)
                        .map(|url| json!({"type": "image_url", "image_url": {"url": url}}))
                        .ok_or_else(AttemptFailure::invalid_request),
                    _ => Err(AttemptFailure::invalid_request()),
                }
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(AttemptFailure::invalid_request()),
    }
}

fn translate_tools(tools: &Value) -> Result<Value, AttemptFailure> {
    let tools = tools
        .as_array()
        .ok_or_else(AttemptFailure::invalid_request)?;
    tools
        .iter()
        .map(|tool| {
            if tool.get("type").and_then(Value::as_str) != Some("function") {
                return Err(AttemptFailure::invalid_request());
            }
            if let Some(function) = tool.get("function") {
                return Ok(json!({"type": "function", "function": function}));
            }
            let name = tool
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::invalid_request)?;
            let mut function =
                serde_json::Map::from_iter([("name".to_string(), Value::String(name.to_string()))]);
            for field in ["description", "parameters", "strict"] {
                if let Some(value) = tool.get(field) {
                    function.insert(field.to_string(), value.clone());
                }
            }
            Ok(json!({"type": "function", "function": function}))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn translate_tool_choice(tool_choice: &Value) -> Result<Value, AttemptFailure> {
    if tool_choice.is_string() {
        return Ok(tool_choice.clone());
    }
    let name = tool_choice
        .get("name")
        .or_else(|| {
            tool_choice
                .get("function")
                .and_then(|value| value.get("name"))
        })
        .and_then(Value::as_str)
        .ok_or_else(AttemptFailure::invalid_request)?;
    Ok(json!({"type": "function", "function": {"name": name}}))
}

fn translate_chat_response(body: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AttemptFailure::translation())?;
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(AttemptFailure::translation)?;
    let message = choice
        .get("message")
        .and_then(Value::as_object)
        .ok_or_else(AttemptFailure::translation)?;
    let id = response
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("chat-response");
    let model = response
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let mut output = Vec::new();
    if let Some(content) = message.get("content").and_then(Value::as_str) {
        output.push(json!({
            "id": format!("{id}-message"),
            "type": "message",
            "status": "completed",
            "role": "assistant",
            "content": [{"type": "output_text", "text": content, "annotations": []}],
        }));
    }
    if let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in tool_calls {
            let function = call
                .get("function")
                .and_then(Value::as_object)
                .ok_or_else(AttemptFailure::translation)?;
            let call_id = call
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(AttemptFailure::translation)?;
            output.push(json!({
                "id": call_id,
                "type": "function_call",
                "status": "completed",
                "call_id": call_id,
                "name": function.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::translation)?,
                "arguments": function.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
            }));
        }
    }
    if output.is_empty() {
        return Err(AttemptFailure::translation());
    }
    let usage = response.get("usage").map(|usage| {
        json!({
            "input_tokens": usage.get("prompt_tokens").or_else(|| usage.get("input_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "output_tokens": usage.get("completion_tokens").or_else(|| usage.get("output_tokens")).and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    serde_json::to_vec(&json!({
        "id": id,
        "object": "response",
        "created_at": response.get("created").and_then(Value::as_u64).unwrap_or(0),
        "status": "completed",
        "model": model,
        "output": output,
        "usage": usage,
    }))
    .map_err(|_| AttemptFailure::translation())
}

fn translate_responses_response(body: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    let response: Value =
        serde_json::from_slice(body).map_err(|_| AttemptFailure::translation())?;
    let output = response
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(AttemptFailure::translation)?;
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    for item in output {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for part in content {
                        if matches!(
                            part.get("type").and_then(Value::as_str),
                            Some("output_text" | "text")
                        ) {
                            if let Some(value) = part.get("text").and_then(Value::as_str) {
                                text.push_str(value);
                            }
                        }
                    }
                }
            }
            Some("function_call") => {
                let call_id = item
                    .get("call_id")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .ok_or_else(AttemptFailure::translation)?;
                tool_calls.push(json!({
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": item.get("name").and_then(Value::as_str).ok_or_else(AttemptFailure::translation)?,
                        "arguments": item.get("arguments").and_then(Value::as_str).unwrap_or("{}"),
                    }
                }));
            }
            _ => {}
        }
    }
    if text.is_empty() && tool_calls.is_empty() {
        return Err(AttemptFailure::translation());
    }
    let mut message = serde_json::Map::from_iter([
        ("role".to_string(), Value::String("assistant".to_string())),
        (
            "content".to_string(),
            if text.is_empty() {
                Value::Null
            } else {
                Value::String(text)
            },
        ),
    ]);
    if !tool_calls.is_empty() {
        message.insert("tool_calls".to_string(), Value::Array(tool_calls));
    }
    let usage = response.get("usage").map(|usage| {
        json!({
            "prompt_tokens": usage.get("input_tokens").and_then(Value::as_u64).unwrap_or(0),
            "completion_tokens": usage.get("output_tokens").and_then(Value::as_u64).unwrap_or(0),
            "total_tokens": usage.get("total_tokens").and_then(Value::as_u64).unwrap_or(0),
        })
    });
    serde_json::to_vec(&json!({
        "id": response.get("id").and_then(Value::as_str).unwrap_or("response"),
        "object": "chat.completion",
        "created": response.get("created_at").and_then(Value::as_u64).unwrap_or(0),
        "model": response.get("model").and_then(Value::as_str).unwrap_or("unknown"),
        "choices": [{
            "index": 0,
            "message": Value::Object(message),
            "finish_reason": if response.get("status").and_then(Value::as_str) == Some("incomplete") { "length" } else if response.get("output").and_then(Value::as_array).is_some_and(|output| output.iter().any(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))) { "tool_calls" } else { "stop" },
        }],
        "usage": usage,
    }))
    .map_err(|_| AttemptFailure::translation())
}

fn completed_sse(response: &[u8]) -> Bytes {
    let response = serde_json::from_slice::<Value>(response).unwrap_or(Value::Null);
    Bytes::from(format!(
        "data: {}\n\n",
        json!({"type": "response.completed", "response": response})
    ))
}

fn completed_chat_sse(response: &[u8]) -> Bytes {
    let Ok(response) = serde_json::from_slice::<Value>(response) else {
        return Bytes::new();
    };
    let choice = response
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .cloned()
        .unwrap_or(Value::Null);
    let common = json!({
        "id": response.get("id").cloned().unwrap_or(Value::Null),
        "object": "chat.completion.chunk",
        "created": response.get("created").cloned().unwrap_or(Value::from(0)),
        "model": response.get("model").cloned().unwrap_or(Value::Null),
    });
    let mut first = common.clone();
    first["choices"] = json!([{
        "index": 0,
        "delta": choice.get("message").cloned().unwrap_or(Value::Null),
        "finish_reason": Value::Null,
    }]);
    let mut terminal = common;
    terminal["choices"] = json!([{
        "index": 0,
        "delta": {},
        "finish_reason": choice.get("finish_reason").cloned().unwrap_or(Value::String("stop".to_string())),
    }]);
    terminal["usage"] = response.get("usage").cloned().unwrap_or(Value::Null);
    Bytes::from(format!(
        "data: {first}\n\ndata: {terminal}\n\ndata: [DONE]\n\n"
    ))
}

#[derive(Clone, Copy)]
struct AttemptFailure {
    status: StatusCode,
    category: &'static str,
    message: &'static str,
    cooldown_ms: u64,
}

struct FailureState {
    cooldown_scope: String,
    retry_at_ms: u64,
    consecutive_failures: u32,
}

impl AttemptFailure {
    fn transport() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_transport",
            message: "upstream request failed",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
        }
    }

    fn body() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_error",
            message: "upstream response failed",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
        }
    }

    fn invalid_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "invalid_request",
            message: "request cannot be translated for an eligible source",
            cooldown_ms: 0,
        }
    }

    fn translation() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_translation",
            message: "upstream response could not be translated",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
        }
    }

    fn status(status: StatusCode) -> Self {
        Self {
            status,
            category: "upstream_status",
            message: "all eligible upstream sources failed",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
        }
    }

    fn stream(category: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category,
            message: "upstream stream failed before the first event",
            cooldown_ms: TRANSIENT_COOLDOWN_MS,
        }
    }

    fn no_candidate() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "no_eligible_source",
            message: "no eligible source is available for this model",
            cooldown_ms: 0,
        }
    }

    fn prepare(error: ExecutorPrepareError) -> Self {
        match error {
            ExecutorPrepareError::Authentication | ExecutorPrepareError::InvalidCredential => {
                Self {
                    status: StatusCode::UNAUTHORIZED,
                    category: "account_auth",
                    message: "account authorization is unavailable",
                    cooldown_ms: 30 * 60_000,
                }
            }
            ExecutorPrepareError::Persistence => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                category: "account_token_persistence",
                message: "refreshed account authorization could not be persisted",
                cooldown_ms: TRANSIENT_COOLDOWN_MS,
            },
            ExecutorPrepareError::Transient => Self {
                status: StatusCode::BAD_GATEWAY,
                category: "account_refresh",
                message: "account authorization refresh failed",
                cooldown_ms: TRANSIENT_COOLDOWN_MS,
            },
        }
    }
}

fn retryable_status(status: StatusCode, has_previous_response_id: bool) -> bool {
    matches!(
        status,
        StatusCode::UNAUTHORIZED
            | StatusCode::PAYMENT_REQUIRED
            | StatusCode::FORBIDDEN
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    ) || (status == StatusCode::NOT_FOUND && !has_previous_response_id)
}

fn apply_status_cooldown(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    status: StatusCode,
    headers: &reqwest::header::HeaderMap,
) -> FailureState {
    let consecutive_failures = runtime.record_failure(candidate_id);
    let now_system = SystemTime::now();
    let now = now_system
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
    let (scope, duration_ms) = match status {
        StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN => {
            ("*", 30 * 60_000)
        }
        StatusCode::NOT_FOUND => (model, 12 * 60 * 60_000),
        StatusCode::TOO_MANY_REQUESTS => (
            model,
            retry_after_ms(headers, now_system)
                .unwrap_or_else(|| exponential_backoff_ms(consecutive_failures)),
        ),
        _ => ("*", TRANSIENT_COOLDOWN_MS),
    };
    let retry_at_ms = now.saturating_add(duration_ms);
    runtime.set_cooldown(candidate_id, scope, retry_at_ms);
    FailureState {
        cooldown_scope: scope.to_string(),
        retry_at_ms,
        consecutive_failures,
    }
}

fn apply_cooldown(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    scope: &str,
    duration_ms: u64,
) -> FailureState {
    let consecutive_failures = runtime.record_failure(candidate_id);
    let retry_at_ms = now_ms().saturating_add(duration_ms);
    runtime.set_cooldown(candidate_id, scope, retry_at_ms);
    FailureState {
        cooldown_scope: scope.to_string(),
        retry_at_ms,
        consecutive_failures,
    }
}

fn apply_failure_state(event: &mut UsageEvent, state: FailureState) {
    event.cooldown_scope = Some(state.cooldown_scope);
    event.retry_at_ms = Some(state.retry_at_ms);
    event.consecutive_failures = Some(state.consecutive_failures);
}

fn retry_after_ms(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<u64> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    let duration_ms = if let Ok(seconds) = value.parse::<u64>() {
        seconds.saturating_mul(1_000)
    } else {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .ok()?
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    };
    Some(duration_ms.min(MAX_RATE_LIMIT_COOLDOWN_MS))
}

fn exponential_backoff_ms(consecutive_failures: u32) -> u64 {
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    1_000_u64
        .saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX))
        .min(MAX_RATE_LIMIT_COOLDOWN_MS)
}

fn affinity_session(headers: &HeaderMap, request: &Value) -> Option<String> {
    request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .or_else(|| {
            request
                .get("metadata")
                .and_then(|metadata| metadata.get("user_id"))
                .and_then(Value::as_str)
        })
        .or_else(|| header_value(headers, "x-session-id"))
        .or_else(|| header_value(headers, "session_id"))
        .or_else(|| header_value(headers, "x-amp-thread-id"))
        .or_else(|| header_value(headers, "x-client-request-id"))
        .or_else(|| request.get("conversation_id").and_then(Value::as_str))
        .map(str::to_string)
}

fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .filter(|value| !value.trim().is_empty())
}

fn candidate_protocols(wire_api: WireApi) -> &'static [WireApi] {
    match wire_api {
        WireApi::Responses => &[WireApi::Responses, WireApi::ChatCompletions],
        WireApi::ChatCompletions => &[WireApi::ChatCompletions, WireApi::Responses],
        WireApi::Messages => &[WireApi::Messages],
    }
}

fn valid_local_host(headers: &HeaderMap) -> bool {
    let Some(host) = headers
        .get(HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<axum::http::uri::Authority>().ok())
    else {
        return false;
    };
    host.host().eq_ignore_ascii_case("localhost")
        || host
            .host()
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn invalid_host() -> Response<Body> {
    api_error(
        StatusCode::MISDIRECTED_REQUEST,
        "Host must target the local gateway",
        "invalid_host",
    )
}

fn unauthorized() -> Response<Body> {
    let mut response = api_error(
        StatusCode::UNAUTHORIZED,
        "local API key is missing or invalid",
        "invalid_api_key",
    );
    response
        .headers_mut()
        .insert(WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    response
}

fn cooldown_error(retry_at_ms: u64) -> Response<Body> {
    let seconds = retry_at_ms
        .saturating_sub(now_ms())
        .saturating_add(999)
        .checked_div(1_000)
        .unwrap_or_default()
        .max(1);
    let mut response = api_error(
        StatusCode::TOO_MANY_REQUESTS,
        "all eligible sources are cooling down",
        "all_sources_cooling_down",
    );
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

fn api_error(status: StatusCode, message: &str, code: &str) -> Response<Body> {
    (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": "relay_error",
                "code": code,
            }
        })),
    )
        .into_response()
}

fn proxy_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    for name in [CONTENT_TYPE, CACHE_CONTROL] {
        if let Some(value) = upstream_headers.get(name.as_str()) {
            response.headers_mut().insert(name, value.clone());
        }
    }
    response
}

fn proxy_sse_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    if let Some(value) = upstream_headers.get(CACHE_CONTROL.as_str()) {
        response.headers_mut().insert(CACHE_CONTROL, value.clone());
    }
    response
}

fn proxy_json_response(
    status: reqwest::StatusCode,
    upstream_headers: &reqwest::header::HeaderMap,
    body: Body,
) -> Response<Body> {
    let mut response = Response::builder().status(status).body(body).unwrap();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(value) = upstream_headers.get(CACHE_CONTROL.as_str()) {
        response.headers_mut().insert(CACHE_CONTROL, value.clone());
    }
    response
}

#[allow(clippy::too_many_arguments)]
fn usage_event(
    request_id: &str,
    attempt: u16,
    local_key_id: &str,
    route: &ExecutorRoute,
    requested_model: &str,
    success: bool,
    http_status: u16,
    error_category: Option<String>,
    latency_ms: u64,
) -> UsageEvent {
    UsageEvent {
        request_id: request_id.to_string(),
        attempt,
        local_key_id: local_key_id.to_string(),
        source_id: route.source_id.clone(),
        candidate_id: Some(route.candidate_id.clone()),
        account_id: route.account_id.clone(),
        requested_model: Some(requested_model.to_string()),
        resolved_model: Some(route.source_model.clone()),
        wire_api: route.wire_api,
        success,
        http_status,
        error_category,
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
        latency_ms,
        ttft_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
    }
}

fn populate_tokens(event: &mut UsageEvent, body: &[u8]) {
    let Ok(body) = serde_json::from_slice::<Value>(body) else {
        return;
    };
    let Some(usage) = find_usage(&body) else {
        return;
    };
    apply_usage(event, usage);
}

fn emit_usage(runtime: &GatewayRuntime, event: UsageEvent) {
    emit_callback(&runtime.usage, event);
}

fn emit_callback(callback: &crate::UsageCallback, event: UsageEvent) {
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(event)));
}

fn request_id() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("relay-{timestamp}-{sequence}")
}

struct UsageStream<S> {
    inner: Pin<Box<S>>,
    callback: crate::UsageCallback,
    completion: CompletionCallback,
    event: Option<UsageEvent>,
    started: Instant,
    sse_pending: Vec<u8>,
    output_pending: VecDeque<Bytes>,
    terminated: bool,
}

impl<S> UsageStream<S> {
    fn new(
        stream: S,
        callback: crate::UsageCallback,
        event: UsageEvent,
        started: Instant,
        completion: CompletionCallback,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            callback,
            completion,
            event: Some(event),
            started,
            sse_pending: Vec::new(),
            output_pending: VecDeque::new(),
            terminated: false,
        }
    }

    fn finish(&mut self, success: Option<bool>, category: Option<&str>) {
        let Some(mut event) = self.event.take() else {
            return;
        };
        if let Some(success) = success {
            event.success = success;
        }
        if let Some(category) = category {
            event.error_category = Some(category.to_string());
        }
        event.latency_ms = self.started.elapsed().as_millis() as u64;
        (self.completion)(&mut event);
        emit_callback(&self.callback, event);
    }

    fn ingest_sse(&mut self, bytes: &[u8]) {
        if self.terminated {
            return;
        }
        if self.sse_pending.len().saturating_add(bytes.len()) > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.finish(Some(false), Some("stream_event_too_large"));
            self.terminated = true;
            return;
        }
        self.sse_pending.extend_from_slice(bytes);
        while let Some(end) = sse_event_end(&self.sse_pending) {
            if end > MAX_SSE_EVENT_BYTES {
                self.sse_pending.clear();
                self.finish(Some(false), Some("stream_event_too_large"));
                self.terminated = true;
                return;
            }
            let event = self.sse_pending.drain(..end).collect::<Vec<_>>();
            let terminal = parse_sse_event(&event);
            if terminal.has_data && !terminal.valid {
                self.sse_pending.clear();
                self.finish(Some(false), Some("stream_invalid"));
                self.terminated = true;
                return;
            }
            if terminal.has_data
                && self
                    .event
                    .as_ref()
                    .is_some_and(|event| event.ttft_ms.is_none())
            {
                if let Some(current) = self.event.as_mut() {
                    current.ttft_ms = Some(self.started.elapsed().as_millis() as u64);
                }
            }
            if let Some(usage) = terminal.usage {
                if let Some(current) = self.event.as_mut() {
                    apply_usage(current, &usage);
                }
            }
            self.output_pending.push_back(Bytes::from(event));
            match terminal.outcome {
                Some(TerminalOutcome::Success) => {
                    self.finish(None, None);
                    self.terminated = true;
                    return;
                }
                Some(TerminalOutcome::Failure) => {
                    self.finish(Some(false), Some("upstream_terminal"));
                    self.terminated = true;
                    return;
                }
                None => {}
            }
        }
        if self.sse_pending.len() > MAX_SSE_EVENT_BYTES {
            self.sse_pending.clear();
            self.finish(Some(false), Some("stream_event_too_large"));
            self.terminated = true;
        }
    }
}

impl<S, E> Stream for UsageStream<S>
where
    S: Stream<Item = std::result::Result<Bytes, E>>,
{
    type Item = std::result::Result<Bytes, E>;

    fn poll_next(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        loop {
            if let Some(bytes) = this.output_pending.pop_front() {
                if this
                    .event
                    .as_ref()
                    .is_some_and(|event| event.ttft_ms.is_none() && !bytes.is_empty())
                {
                    if let Some(event) = this.event.as_mut() {
                        event.ttft_ms = Some(this.started.elapsed().as_millis() as u64);
                    }
                }
                return Poll::Ready(Some(Ok(bytes)));
            }
            if this.terminated {
                return Poll::Ready(None);
            }
            match this.inner.as_mut().poll_next(context) {
                Poll::Ready(Some(Ok(bytes))) => this.ingest_sse(&bytes),
                Poll::Ready(Some(Err(error))) => {
                    this.finish(Some(false), Some("upstream_stream"));
                    this.terminated = true;
                    return Poll::Ready(Some(Err(error)));
                }
                Poll::Ready(None) => {
                    if this.event.as_ref().is_some_and(|event| event.success) {
                        this.finish(Some(false), Some("stream_incomplete"));
                    } else {
                        this.finish(None, None);
                    }
                    this.sse_pending.clear();
                    this.terminated = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[derive(Default)]
struct TerminalEvent {
    has_data: bool,
    valid: bool,
    outcome: Option<TerminalOutcome>,
    usage: Option<Value>,
    response: Option<Value>,
    output_item: Option<Value>,
}

#[derive(Debug, Eq, PartialEq)]
enum TerminalOutcome {
    Success,
    Failure,
}

fn sse_event_end(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(2)
        .position(|window| window == b"\n\n")
        .map(|position| position + 2)
        .or_else(|| {
            bytes
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
        })
}

fn parse_sse_event(event: &[u8]) -> TerminalEvent {
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
    if data.is_empty() {
        return TerminalEvent::default();
    }
    if data == b"[DONE]" {
        return TerminalEvent {
            has_data: true,
            valid: true,
            outcome: Some(TerminalOutcome::Success),
            usage: None,
            response: None,
            output_item: None,
        };
    }
    let Ok(value) = serde_json::from_slice::<Value>(&data) else {
        return TerminalEvent {
            has_data: true,
            ..TerminalEvent::default()
        };
    };
    let outcome = match value.get("type").and_then(Value::as_str) {
        Some("response.completed" | "response.done") => Some(TerminalOutcome::Success),
        Some("response.failed" | "response.incomplete" | "error") => Some(TerminalOutcome::Failure),
        _ => None,
    };
    let usage = find_usage(&value).cloned();
    let response = value.get("response").cloned();
    let output_item = (value.get("type").and_then(Value::as_str)
        == Some("response.output_item.done"))
    .then(|| value.get("item").cloned())
    .flatten();
    TerminalEvent {
        has_data: true,
        valid: true,
        outcome,
        usage,
        response,
        output_item,
    }
}

fn completed_account_response(bytes: &[u8]) -> Result<Vec<u8>, AttemptFailure> {
    if serde_json::from_slice::<Value>(bytes).is_ok() {
        return Ok(bytes.to_vec());
    }
    let mut offset = 0;
    let mut output = Vec::new();
    while let Some(end) = sse_event_end(&bytes[offset..]) {
        let terminal = parse_sse_event(&bytes[offset..offset + end]);
        if terminal.has_data && !terminal.valid {
            return Err(AttemptFailure::stream("stream_invalid"));
        }
        if let Some(item) = terminal.output_item {
            output.push(item);
        }
        match terminal.outcome {
            Some(TerminalOutcome::Failure) => {
                return Err(AttemptFailure::stream("upstream_terminal"));
            }
            Some(TerminalOutcome::Success) => {
                if let Some(mut response) = terminal.response {
                    if response
                        .get("output")
                        .and_then(Value::as_array)
                        .is_some_and(Vec::is_empty)
                    {
                        response["output"] = Value::Array(output);
                    }
                    return serde_json::to_vec(&response)
                        .map_err(|_| AttemptFailure::stream("stream_invalid"));
                }
            }
            None => {}
        }
        offset += end;
    }
    Err(AttemptFailure::stream("stream_incomplete"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

fn apply_usage(event: &mut UsageEvent, usage: &Value) {
    let input_tokens = usage
        .get("input_tokens")
        .or_else(|| usage.get("prompt_tokens"))
        .and_then(Value::as_u64);
    let output_tokens = usage
        .get("output_tokens")
        .or_else(|| usage.get("completion_tokens"))
        .and_then(Value::as_u64);
    event.input_tokens = input_tokens;
    event.cached_input_tokens = usage
        .get("input_tokens_details")
        .and_then(|details| details.get("cached_tokens"))
        .or_else(|| {
            usage
                .get("prompt_tokens_details")
                .and_then(|details| details.get("cached_tokens"))
        })
        .or_else(|| usage.get("cached_tokens"))
        .and_then(Value::as_u64)
        .map(|cached| cached.min(input_tokens.unwrap_or(cached)));
    event.reasoning_tokens = usage
        .get("reasoning_tokens")
        .or_else(|| {
            usage
                .get("output_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .or_else(|| {
            usage
                .get("completion_tokens_details")
                .and_then(|details| details.get("reasoning_tokens"))
        })
        .and_then(Value::as_u64)
        .map(|reasoning| reasoning.min(output_tokens.unwrap_or(reasoning)));
    event.output_tokens = output_tokens;
    let reported_total = usage.get("total_tokens").and_then(Value::as_u64);
    event.total_tokens = match (input_tokens, output_tokens) {
        (Some(input), Some(output)) => {
            let measured = input.saturating_add(output);
            Some(reported_total.unwrap_or(measured).max(measured))
        }
        _ => reported_total,
    };
}

fn find_usage(value: &Value) -> Option<&Value> {
    value.get("usage").or_else(|| {
        let response = value.get("response")?;
        response.get("usage").or_else(|| {
            response
                .get("response")
                .and_then(|nested| nested.get("usage"))
        })
    })
}

impl<S> Drop for UsageStream<S> {
    fn drop(&mut self) {
        self.finish(Some(false), Some("client_cancelled"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{convert::Infallible, sync::Mutex, time::Duration};

    #[test]
    fn oversized_sse_event_is_recorded_as_failure() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            UsageEvent {
                request_id: "request".into(),
                attempt: 1,
                local_key_id: "key".into(),
                source_id: "source".into(),
                candidate_id: Some("source".into()),
                account_id: None,
                requested_model: Some("model".into()),
                resolved_model: Some("model".into()),
                wire_api: crate::WireApi::Responses,
                success: true,
                http_status: 200,
                error_category: None,
                cooldown_scope: None,
                retry_at_ms: None,
                consecutive_failures: Some(0),
                latency_ms: 0,
                ttft_ms: None,
                input_tokens: None,
                cached_input_tokens: None,
                reasoning_tokens: None,
                output_tokens: None,
                total_tokens: None,
            },
            Instant::now(),
            Arc::new(|_| {}),
        );
        stream.ingest_sse(&vec![b'x'; MAX_SSE_EVENT_BYTES + 1]);
        assert!(stream.terminated);
        assert!(stream.sse_pending.is_empty());
        assert!(stream.output_pending.is_empty());
        drop(stream);

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(
            events[0].error_category.as_deref(),
            Some("stream_event_too_large")
        );
    }

    #[test]
    fn non_stream_usage_normalizes_cached_reasoning_and_total_tokens() {
        let mut event = test_usage_event();
        populate_tokens(
            &mut event,
            br#"{"response":{"response":{"usage":{"input_tokens":16,"input_tokens_details":{"cached_tokens":30},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":30},"total_tokens":10}}}}"#,
        );

        assert_eq!(event.input_tokens, Some(16));
        assert_eq!(event.cached_input_tokens, Some(16));
        assert_eq!(event.reasoning_tokens, Some(5));
        assert_eq!(event.output_tokens, Some(5));
        assert_eq!(event.total_tokens, Some(21));
    }

    #[test]
    fn streaming_chat_usage_captures_cached_prompt_tokens() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut stream = UsageStream::new(
            futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_| {}),
        );
        stream.ingest_sse(
            b"data: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"prompt_tokens\":32,\"prompt_tokens_details\":{\"cached_tokens\":9},\"completion_tokens\":6,\"completion_tokens_details\":{\"reasoning_tokens\":4}}}}\n\n",
        );

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].input_tokens, Some(32));
        assert_eq!(events[0].cached_input_tokens, Some(9));
        assert_eq!(events[0].reasoning_tokens, Some(4));
        assert_eq!(events[0].output_tokens, Some(6));
        assert_eq!(events[0].total_tokens, Some(38));
    }

    #[test]
    fn all_responses_error_terminal_types_are_failures() {
        for event_type in ["response.failed", "response.incomplete", "error"] {
            let event = format!("data: {{\"type\":\"{event_type}\"}}\n\n");
            assert_eq!(
                parse_sse_event(event.as_bytes()).outcome,
                Some(TerminalOutcome::Failure)
            );
        }
    }

    #[test]
    fn retry_after_supports_delta_seconds_and_http_dates() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, reqwest::header::HeaderValue::from_static("17"));
        assert_eq!(retry_after_ms(&headers, now), Some(17_000));

        let date = httpdate::fmt_http_date(now + Duration::from_secs(23));
        headers.insert(RETRY_AFTER, date.parse().unwrap());
        assert_eq!(retry_after_ms(&headers, now), Some(23_000));
    }

    #[test]
    fn no_header_rate_limit_backoff_is_exponential_and_capped() {
        assert_eq!(exponential_backoff_ms(1), 1_000);
        assert_eq!(exponential_backoff_ms(2), 2_000);
        assert_eq!(exponential_backoff_ms(3), 4_000);
        assert_eq!(exponential_backoff_ms(32), MAX_RATE_LIMIT_COOLDOWN_MS);
    }

    fn test_usage_event() -> UsageEvent {
        UsageEvent {
            request_id: "request".into(),
            attempt: 1,
            local_key_id: "key".into(),
            source_id: "source".into(),
            candidate_id: Some("source".into()),
            account_id: None,
            requested_model: Some("model".into()),
            resolved_model: Some("model".into()),
            wire_api: crate::WireApi::Responses,
            success: true,
            http_status: 200,
            error_category: None,
            cooldown_scope: None,
            retry_at_ms: None,
            consecutive_failures: Some(0),
            latency_ms: 0,
            ttft_ms: None,
            input_tokens: None,
            cached_input_tokens: None,
            reasoning_tokens: None,
            output_tokens: None,
            total_tokens: None,
        }
    }
}
