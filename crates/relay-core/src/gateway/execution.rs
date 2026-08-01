use super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::errors::{
    api_error, apply_attempt_failure_cooldown, apply_cooldown, apply_failure_cooldown_with_body,
    apply_failure_cooldown_with_hint, apply_failure_state, cooldown_error,
    failure_category_is_request_terminal, failure_category_requires_cooldown,
    previous_response_not_found, recoverable_response_affinity_miss, retry_candidate_limit,
    retryable_failure, retryable_status, AttemptFailure, TRANSIENT_COOLDOWN_MS,
};
use super::now_ms;
use super::request::{
    account_endpoint_url, candidate_protocols, contains_tool_call_output, forwarded_codex_headers,
    normalize_account_request, normalize_account_request_body, normalize_service_tier, request_id,
    request_service_tier, try_recover_encrypted_content, AccountEndpoint,
    CODEX_RESPONSES_LITE_HEADER, MAX_CLIENT_REQUEST_BODY_BYTES, MAX_CLIENT_REQUEST_BODY_ERROR,
};
use super::response::{
    completed_account_response, emit_usage, populate_tokens, proxy_json_response, proxy_response,
    proxy_sse_response, response_id_from_bytes, upstream_body_error_response, usage_event,
    CompletionCallback,
};
use super::streaming::{bootstrap_stream, UsageStream};
use super::translation::{
    completed_chat_sse, completed_sse, replay_responses_request, responses_replay_seed,
    translate_chat_request, translate_chat_response, translate_responses_request,
    translate_responses_response,
};
use crate::protocol::ClientWireApi;
use crate::runtime::AuthenticatedKey;
use crate::{Error, GatewayRuntime, WireApi};
use axum::body::{Body, Bytes};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use futures_util::{stream, StreamExt};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

#[allow(clippy::too_many_arguments)]
pub(super) async fn execute_account_endpoint(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    mut request: Value,
    requested_model: String,
    resolved_model: String,
    client_headers: HeaderMap,
    endpoint: AccountEndpoint,
    responses_lite: Option<HeaderValue>,
    response_affinity_key: Option<String>,
    rewrite_model: bool,
) -> Response<Body> {
    let request_id = request_id();
    let service_tier = request_service_tier(&request);
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
    );
    let has_previous_response_id = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let account_only_exclusions = runtime.api_source_candidate_ids();
    let mut tried = account_only_exclusions.clone();
    let mut attempt = 0_u16;
    let mut owner_recovery_confirmed = false;
    let mut exhaust_pool = false;
    let mut encrypted_content_recovered = false;
    let mut last_failure = None;

    while exhaust_pool
        || usize::from(attempt)
            < retry_candidate_limit(runtime.max_retry_candidates(), owner_recovery_confirmed)
                + usize::from(encrypted_content_recovered)
    {
        let Some((selected, lease)) = runtime.select_and_reserve(
            &key,
            &resolved_model,
            &[WireApi::Responses],
            &tried,
            (
                response_affinity_key.as_deref(),
                prompt_affinity_key.as_deref(),
            ),
            now_ms(),
        ) else {
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let Some(mut route) = runtime.executor_route(&selected.candidate_id, &resolved_model)
        else {
            continue;
        };
        if route.account_id.is_none() {
            continue;
        }
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.service_tier = service_tier;
        let Some(upstream_url) = account_endpoint_url(route.upstream_url.clone(), endpoint) else {
            last_failure = Some(AttemptFailure::invalid_request());
            continue;
        };
        let mut upstream_body = request.clone();
        if rewrite_model {
            upstream_body.as_object_mut().unwrap().insert(
                "model".to_string(),
                Value::String(route.source_model.clone()),
            );
        }
        let request_body = match serde_json::to_vec(&upstream_body) {
            Ok(body) => body,
            Err(_) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "request body could not be serialized",
                    "invalid_request",
                )
            }
        };

        attempt = attempt.saturating_add(1);
        let started = Instant::now();
        let mut upstream_request = runtime
            .request_client(&route.candidate_id, false)
            .post(upstream_url)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json")
            .headers(forwarded_codex_headers(&client_headers, &request_id));
        if endpoint == AccountEndpoint::Compact {
            if let Some(value) = responses_lite.as_ref() {
                upstream_request =
                    upstream_request.header(CODEX_RESPONSES_LITE_HEADER, value.clone());
            }
        }
        let upstream = runtime
            .send_authorized_request(
                &route.candidate_id,
                upstream_request.body(request_body),
                None,
            )
            .await;
        let upstream = match upstream {
            Ok(upstream) => upstream,
            Err(error) => {
                let failure = AttemptFailure::authorized_request(error);
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    failure.cooldown_ms,
                    route.half_open_probe,
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
        };
        let status = upstream.status();
        exhaust_pool |= status == StatusCode::TOO_MANY_REQUESTS;
        let response_headers = upstream.headers().clone();
        let bytes = match crate::runtime::collect_limited(upstream, endpoint.response_limit()).await
        {
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
        if !status.is_success() {
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            let mut event = usage_event(
                &request_id,
                attempt,
                &key.id,
                &route,
                &requested_model,
                false,
                status.as_u16(),
                Some(failure.category.to_string()),
                started.elapsed().as_millis() as u64,
            );
            if failure.category == "upstream_encrypted_content_invalid"
                && try_recover_encrypted_content(&mut request, &mut encrypted_content_recovered)
            {
                tried.remove(&route.candidate_id);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
            let affinity_miss = recoverable_response_affinity_miss(
                status,
                has_previous_response_id,
                response_affinity_hit,
                previous_response_not_found(&bytes),
            );
            if affinity_miss
                || retryable_failure(status, failure.category, has_previous_response_id)
            {
                if affinity_miss {
                    owner_recovery_confirmed |= !response_affinity_hit;
                    runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                    event.error_category = Some("response_affinity_miss".to_string());
                } else {
                    let state = apply_failure_cooldown_with_body(
                        &runtime,
                        &route.candidate_id,
                        &route.source_model,
                        status,
                        failure.category,
                        &response_headers,
                        Some(&bytes),
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                }
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
            emit_usage(&runtime, event);
            return proxy_response(status, &response_headers, Body::from(bytes));
        }

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
        populate_tokens(&mut event, &bytes);
        let recovered = runtime.record_success_with_metrics(
            &route.candidate_id,
            &route.source_model,
            now_ms(),
            event.output_tokens,
            event.generation_ms.unwrap_or(event.latency_ms),
        );
        event.consecutive_failures = recovered.then_some(0);
        runtime.bind_prompt_affinity(
            prompt_affinity_key.as_deref(),
            &route.candidate_id,
            now_ms(),
        );
        emit_usage(&runtime, event);
        drop(lease);
        return proxy_response(status, &response_headers, Body::from(bytes));
    }

    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(retry_at) = runtime.earliest_retry_at(
            &key,
            &resolved_model,
            &[WireApi::Responses],
            &account_only_exclusions,
            response_affinity_key.as_deref(),
            now_ms(),
        ) {
            return cooldown_error(retry_at, Some(&failure));
        }
    }
    api_error(failure.status, failure.message, failure.category)
}

pub(super) async fn execute_client_request(
    runtime: Arc<GatewayRuntime>,
    request: Request<Body>,
    wire_api: WireApi,
) -> Response<Body> {
    let (parts, body) = request.into_parts();
    let headers = parts.headers;
    if !valid_local_host(&headers) {
        return invalid_host();
    }
    let Some(key) = runtime.authenticate(headers.get(AUTHORIZATION)) else {
        return unauthorized();
    };
    let client_wire_api = match wire_api {
        WireApi::Responses => ClientWireApi::Responses,
        WireApi::ChatCompletions => ClientWireApi::ChatCompletions,
        WireApi::Messages => return client_api_forbidden(),
    };
    if !runtime.allows_client_wire_api(&key, client_wire_api) {
        return client_api_forbidden();
    }
    let body = match axum::body::to_bytes(body, MAX_CLIENT_REQUEST_BODY_BYTES).await {
        Ok(body) => body,
        Err(_) => {
            return api_error(
                StatusCode::PAYLOAD_TOO_LARGE,
                MAX_CLIENT_REQUEST_BODY_ERROR,
                "request_too_large",
            )
        }
    };

    let mut request: Value = match serde_json::from_slice(&body) {
        Ok(Value::Object(request)) => Value::Object(request),
        _ => {
            return api_error(
                StatusCode::BAD_REQUEST,
                "request body must be a JSON object",
                "invalid_request",
            )
        }
    };
    if let Some(object) = request.as_object_mut() {
        normalize_service_tier(object, runtime.default_service_tier());
    }
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
    let Some(resolved_model) = runtime.resolve_visible_model(
        &key,
        &requested_model,
        candidate_protocols(wire_api),
        now_ms(),
    ) else {
        return api_error(
            StatusCode::NOT_FOUND,
            "model is not available for this local key",
            "model_not_found",
        );
    };
    let responses_lite = headers
        .get(CODEX_RESPONSES_LITE_HEADER)
        .cloned()
        .or_else(|| {
            runtime
                .codex_model_uses_responses_lite(&resolved_model)
                .then(|| HeaderValue::from_static("true"))
        });
    let response_affinity_key =
        runtime.response_affinity_key(request.get("previous_response_id").and_then(Value::as_str));
    let request_id = request_id();
    let forwarded_headers = forwarded_codex_headers(&headers, &request_id);
    execute_request(
        runtime,
        key,
        request,
        requested_model,
        resolved_model,
        stream,
        request_id,
        forwarded_headers,
        response_affinity_key,
        wire_api,
        responses_lite,
        true,
        0,
    )
    .await
}

fn responses_request_for_chat_source(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    request: &Value,
) -> Result<Value, AttemptFailure> {
    let previous_response_id = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|response_id| !response_id.is_empty());
    let Some(previous_response_id) = previous_response_id else {
        return Ok(request.clone());
    };
    if let Some(previous) = runtime.response_replay(&key.id, Some(previous_response_id), now_ms()) {
        return replay_responses_request(&previous, request);
    }
    // Do not silently reset a continuation for a stateless source.  Without
    // the earlier response, that would discard the conversation and can
    // detach a tool result from its call.  A stateful Responses candidate may
    // still own this id; its existing affinity recovery path decides whether a
    // reset is safe after that source confirms the id is gone.
    Err(AttemptFailure::response_replay_unavailable())
}

#[allow(clippy::too_many_arguments)]
async fn execute_request(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    mut request: Value,
    requested_model: String,
    resolved_model: String,
    stream: bool,
    request_id: String,
    forwarded_headers: HeaderMap,
    response_affinity_key: Option<String>,
    wire_api: WireApi,
    responses_lite: Option<HeaderValue>,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
) -> Response<Body> {
    let service_tier = request_service_tier(&request);
    let mut tried = HashSet::new();
    let mut attempt = attempt_offset;
    let mut attempts_this_run = 0_usize;
    let mut owner_recovery_confirmed = false;
    let mut exhaust_pool = false;
    let mut confirmed_response_missing = false;
    let mut encrypted_content_recovered = false;
    let mut last_failure = None;
    let has_previous_response_id = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
    );

    while exhaust_pool
        || attempts_this_run
            < retry_candidate_limit(runtime.max_retry_candidates(), owner_recovery_confirmed)
                + usize::from(encrypted_content_recovered)
    {
        let selected = runtime.select_and_reserve(
            &key,
            &resolved_model,
            candidate_protocols(wire_api),
            &tried,
            (
                response_affinity_key.as_deref(),
                prompt_affinity_key.as_deref(),
            ),
            now_ms(),
        );
        let Some((selected, lease)) = selected else {
            if attempt == 0 {
                if let Some(retry_at) = runtime.earliest_retry_at(
                    &key,
                    &resolved_model,
                    candidate_protocols(wire_api),
                    &tried,
                    response_affinity_key.as_deref(),
                    now_ms(),
                ) {
                    return cooldown_error(retry_at, None);
                }
            }
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let Some(mut route) = runtime.executor_route(&selected.candidate_id, &resolved_model)
        else {
            continue;
        };
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.service_tier = service_tier;
        let source_model = route.source_model.clone();
        let responses_via_chat =
            wire_api == WireApi::Responses && route.wire_api == WireApi::ChatCompletions;
        let chat_via_responses =
            wire_api == WireApi::ChatCompletions && route.wire_api == WireApi::Responses;
        let account_route = route.account_id.is_some();
        let mut replay_request = None;
        let request_body = if responses_via_chat {
            let replay = match responses_request_for_chat_source(&runtime, &key, &request) {
                Ok(replay) => replay,
                Err(failure) => {
                    last_failure = Some(failure);
                    continue;
                }
            };
            let body = match translate_responses_request(&replay, &source_model, false) {
                Ok(body) => body,
                Err(failure) => {
                    last_failure = Some(failure);
                    continue;
                }
            };
            replay_request = Some(replay);
            body
        } else if chat_via_responses {
            match translate_chat_request(&request, &source_model, false) {
                Ok(body) if account_route => {
                    match normalize_account_request_body(&body, responses_lite.is_some()) {
                        Ok(body) => body,
                        Err(failure) => {
                            last_failure = Some(failure);
                            continue;
                        }
                    }
                }
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
                normalize_account_request(object, responses_lite.is_some());
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

        // Cross-protocol adapters translate complete payloads, so stream
        // requests are returned as one terminal SSE sequence.
        let upstream_stream = stream && !responses_via_chat && !chat_via_responses;
        attempt = attempt.saturating_add(1);
        attempts_this_run = attempts_this_run.saturating_add(1);
        let started = Instant::now();
        let client = runtime.request_client(&route.candidate_id, upstream_stream);
        let mut upstream_request = client
            .post(route.upstream_url.clone())
            .header(CONTENT_TYPE, "application/json")
            .headers(forwarded_headers.clone());
        if upstream_stream {
            upstream_request = upstream_request.header(ACCEPT, "text/event-stream");
        }
        if account_route {
            if let Some(value) = responses_lite.as_ref() {
                upstream_request = upstream_request.header(CODEX_RESPONSES_LITE_HEADER, value);
            }
        }
        let upstream = runtime
            .send_authorized_request(
                &route.candidate_id,
                upstream_request.body(request_body),
                None,
            )
            .await;
        let upstream = match upstream {
            Ok(upstream) => upstream,
            Err(error) => {
                let failure = AttemptFailure::authorized_request(error);
                let state = apply_cooldown(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    failure.cooldown_ms,
                    route.half_open_probe,
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
        };

        let status = upstream.status();
        exhaust_pool |= status == StatusCode::TOO_MANY_REQUESTS;
        let response_headers = upstream.headers().clone();
        if !status.is_success() {
            let mut event = usage_event(
                &request_id,
                attempt,
                &key.id,
                &route,
                &requested_model,
                false,
                status.as_u16(),
                None,
                started.elapsed().as_millis() as u64,
            );
            let bytes = match crate::runtime::collect_limited(
                upstream,
                crate::runtime::MAX_NON_STREAM_BODY_BYTES,
            )
            .await
            {
                Ok(bytes) => bytes,
                Err(_) if retryable_status(status, has_previous_response_id) => {
                    let failure = AttemptFailure::status_with_body(status, None);
                    event.error_category = Some(failure.category.to_string());
                    let state = apply_failure_cooldown_with_body(
                        &runtime,
                        &route.candidate_id,
                        &source_model,
                        status,
                        failure.category,
                        &response_headers,
                        None,
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    continue;
                }
                Err(error) => return upstream_body_error_response(&runtime, event, started, error),
            };
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            event.error_category = Some(failure.category.to_string());
            if failure.category == "upstream_encrypted_content_invalid"
                && try_recover_encrypted_content(&mut request, &mut encrypted_content_recovered)
            {
                tried.remove(&route.candidate_id);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
            let response_missing = previous_response_not_found(&bytes);
            let affinity_miss = recoverable_response_affinity_miss(
                status,
                has_previous_response_id,
                response_affinity_hit,
                response_missing,
            );
            if affinity_miss
                || retryable_failure(status, failure.category, has_previous_response_id)
            {
                if affinity_miss {
                    confirmed_response_missing |= response_missing;
                    owner_recovery_confirmed |= !response_affinity_hit;
                    runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                    event.error_category = Some("response_affinity_miss".to_string());
                } else {
                    let state = apply_failure_cooldown_with_body(
                        &runtime,
                        &route.candidate_id,
                        &source_model,
                        status,
                        failure.category,
                        &response_headers,
                        Some(&bytes),
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                }
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                if affinity_miss && response_missing && response_affinity_hit {
                    break;
                }
                continue;
            }
            populate_tokens(&mut event, &bytes);
            emit_usage(&runtime, event);
            return proxy_response(status, &response_headers, Body::from(bytes));
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
                    let state = apply_cooldown(
                        &runtime,
                        &route.candidate_id,
                        "*",
                        TRANSIENT_COOLDOWN_MS,
                        route.half_open_probe,
                    );
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
                        exhaust_pool |= failure.status == StatusCode::TOO_MANY_REQUESTS;
                        let state =
                            failure_category_requires_cooldown(failure.category).then(|| {
                                apply_attempt_failure_cooldown(
                                    &runtime,
                                    &route.candidate_id,
                                    &source_model,
                                    &failure,
                                    &response_headers,
                                    route.half_open_probe,
                                )
                            });
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
                        if failure.category == "upstream_encrypted_content_invalid"
                            && try_recover_encrypted_content(
                                &mut request,
                                &mut encrypted_content_recovered,
                            )
                        {
                            tried.remove(&route.candidate_id);
                            emit_usage(&runtime, event);
                            last_failure = Some(failure);
                            continue;
                        }
                        if let Some(state) = state {
                            apply_failure_state(&mut event, state);
                        }
                        emit_usage(&runtime, event);
                        if failure_category_is_request_terminal(failure.category) {
                            return api_error(failure.status, failure.message, failure.category);
                        }
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else {
                bytes
            };
            let bytes = if responses_via_chat {
                let fallback_response_id = format!("{request_id}-chat-{attempt}");
                match translate_chat_response(&bytes, &fallback_response_id) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let state =
                            failure_category_requires_cooldown(failure.category).then(|| {
                                apply_attempt_failure_cooldown(
                                    &runtime,
                                    &route.candidate_id,
                                    &source_model,
                                    &failure,
                                    &response_headers,
                                    route.half_open_probe,
                                )
                            });
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
                        if let Some(state) = state {
                            apply_failure_state(&mut event, state);
                        }
                        emit_usage(&runtime, event);
                        if failure_category_is_request_terminal(failure.category) {
                            return api_error(failure.status, failure.message, failure.category);
                        }
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else if chat_via_responses {
                match translate_responses_response(&bytes) {
                    Ok(bytes) => bytes,
                    Err(failure) => {
                        let state =
                            failure_category_requires_cooldown(failure.category).then(|| {
                                apply_attempt_failure_cooldown(
                                    &runtime,
                                    &route.candidate_id,
                                    &source_model,
                                    &failure,
                                    &response_headers,
                                    route.half_open_probe,
                                )
                            });
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
                        if let Some(state) = state {
                            apply_failure_state(&mut event, state);
                        }
                        emit_usage(&runtime, event);
                        if failure_category_is_request_terminal(failure.category) {
                            return api_error(failure.status, failure.message, failure.category);
                        }
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else {
                bytes
            };
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
            populate_tokens(&mut event, &bytes);
            let recovered = runtime.record_success_with_metrics(
                &route.candidate_id,
                &source_model,
                now_ms(),
                event.output_tokens,
                event.generation_ms.unwrap_or(event.latency_ms),
            );
            event.consecutive_failures = recovered.then_some(0);
            runtime.bind_prompt_affinity(
                prompt_affinity_key.as_deref(),
                &route.candidate_id,
                now_ms(),
            );
            emit_usage(&runtime, event);
            let completed_response_id = response_id_from_bytes(&bytes);
            if let Some(replay_request) = replay_request.as_ref() {
                if let Ok(response) = serde_json::from_slice::<Value>(&bytes) {
                    if let Ok(replay) = responses_replay_seed(replay_request, &response) {
                        runtime.remember_response_replay(
                            &key.id,
                            completed_response_id.as_deref(),
                            replay,
                            now_ms(),
                        );
                    }
                }
            }
            runtime.bind_response_affinity(
                completed_response_id.as_deref(),
                &route.candidate_id,
                now_ms(),
            );
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
                let completion_prompt_affinity = prompt_affinity_key.clone();
                let completion_half_open_probe = route.half_open_probe;
                let completion_headers = headers.clone();
                let completion: CompletionCallback = Arc::new(move |event, response_id, hint| {
                    lease.release();
                    if event.success {
                        let recovered = completion_runtime.record_success_with_metrics(
                            &completion_source,
                            &completion_model,
                            now_ms(),
                            event.output_tokens,
                            event.generation_ms.unwrap_or(event.latency_ms),
                        );
                        event.consecutive_failures = recovered.then_some(0);
                        completion_runtime.bind_prompt_affinity(
                            completion_prompt_affinity.as_deref(),
                            &completion_source,
                            now_ms(),
                        );
                        completion_runtime.bind_response_affinity(
                            response_id,
                            &completion_source,
                            now_ms(),
                        );
                    } else if let Some(category) = event
                        .error_category
                        .as_deref()
                        .filter(|category| failure_category_requires_cooldown(category))
                    {
                        let status = StatusCode::from_u16(event.http_status)
                            .unwrap_or(StatusCode::BAD_GATEWAY);
                        let state = apply_failure_cooldown_with_hint(
                            &completion_runtime,
                            &completion_source,
                            &completion_model,
                            status,
                            category,
                            &completion_headers,
                            hint,
                            completion_half_open_probe,
                        );
                        apply_failure_state(event, state);
                    }
                });
                let combined =
                    stream::once(async move { Ok::<_, reqwest::Error>(first) }).chain(remaining);
                let usage_stream = UsageStream::with_runtime(
                    combined,
                    runtime.clone(),
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
                exhaust_pool |= failure.status == StatusCode::TOO_MANY_REQUESTS;
                let state = failure_category_requires_cooldown(failure.category).then(|| {
                    apply_attempt_failure_cooldown(
                        &runtime,
                        &route.candidate_id,
                        &source_model,
                        &failure,
                        &response_headers,
                        route.half_open_probe,
                    )
                });
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
                if failure.category == "upstream_encrypted_content_invalid"
                    && try_recover_encrypted_content(&mut request, &mut encrypted_content_recovered)
                {
                    tried.remove(&route.candidate_id);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    continue;
                }
                if let Some(state) = state {
                    apply_failure_state(&mut event, state);
                }
                emit_usage(&runtime, event);
                if failure_category_is_request_terminal(failure.category) {
                    return api_error(failure.status, failure.message, failure.category);
                }
                last_failure = Some(failure);
            }
        }
    }

    if allow_previous_response_reset
        && has_previous_response_id
        && confirmed_response_missing
        && !contains_tool_call_output(&request)
    {
        let mut reset_request = request;
        if let Some(object) = reset_request.as_object_mut() {
            object.remove("previous_response_id");
            return Box::pin(execute_request(
                runtime,
                key,
                reset_request,
                requested_model,
                resolved_model,
                stream,
                request_id,
                forwarded_headers,
                None,
                wire_api,
                responses_lite,
                false,
                attempt,
            ))
            .await;
        }
    }

    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some(retry_at) = runtime.earliest_retry_at(
            &key,
            &resolved_model,
            candidate_protocols(wire_api),
            &HashSet::new(),
            response_affinity_key.as_deref(),
            now_ms(),
        ) {
            return cooldown_error(retry_at, Some(&failure));
        }
    }
    api_error(failure.status, failure.message, failure.category)
}
