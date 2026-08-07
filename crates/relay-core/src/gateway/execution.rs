use super::auth::{client_api_forbidden, invalid_host, unauthorized, valid_local_host};
use super::errors::{
    api_error, apply_attempt_failure_cooldown, apply_cooldown_for_model,
    apply_failure_cooldown_with_body, apply_failure_cooldown_with_hint, apply_failure_state,
    cooldown_error, failure_category_is_request_terminal, failure_category_requires_cooldown,
    failure_requires_independent_source_endpoint, preserved_upstream_error,
    previous_response_not_found, previous_response_requires_websocket,
    recoverable_response_affinity_miss, responses_function_call_output_has_invalid_call_id,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    retry_candidate_limit, retryable_failure, retryable_status, AttemptFailure, CooldownContext,
    PreservedUpstreamError, TRANSIENT_COOLDOWN_MS,
};
use super::now_ms;
use super::request::{
    account_endpoint_url, candidate_protocols, chat_request_is_text_or_image_only,
    chat_request_uses_tools, contains_tool_call_output, forwarded_bridge_messages_headers,
    forwarded_codex_headers, forwarded_messages_headers, normalize_account_request, request_id,
    request_service_tier, tool_use_diagnostics, try_recover_encrypted_content,
    with_forwarded_tool_diagnostics, AccountEndpoint, CODEX_RESPONSES_LITE_HEADER,
    MAX_CLIENT_REQUEST_BODY_BYTES, MAX_CLIENT_REQUEST_BODY_ERROR,
};
use super::response::{
    completed_account_response, emit_usage, populate_tokens, proxy_json_response, proxy_response,
    proxy_sse_response, response_id_from_bytes, upstream_body_error_response, usage_event,
    CompletionCallback,
};
use super::streaming::{bootstrap_stream, bridge_messages_stream, UsageStream};
use crate::protocol::ClientWireApi;
use crate::protocol::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids, AdapterError,
    AdapterRequestContext, MessagesBridgeResponse,
};
use crate::runtime::{AuthenticatedKey, DefaultServiceTier};
use crate::{Error, GatewayRuntime, WireApi};
use axum::body::{Body, Bytes};
use axum::http::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Request, Response, StatusCode};
use futures_util::{stream, Stream, StreamExt};
use serde_json::Value;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
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
    let client_tool_use = tool_use_diagnostics(&request);
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
    let mut encrypted_content_recovered = false;
    let mut function_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
    let mut last_failure = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;

    while usize::from(attempt)
        < retry_candidate_limit(runtime.max_retry_candidates(), owner_recovery_confirmed)
            + usize::from(encrypted_content_recovered)
    {
        let Some((selected, lease)) = runtime
            .select_and_reserve(
                &key,
                &resolved_model,
                &[WireApi::Responses],
                &tried,
                (
                    response_affinity_key.as_deref(),
                    prompt_affinity_key.as_deref(),
                ),
                now_ms(),
            )
            .await
        else {
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let Some(mut route) = runtime.executor_route(
            &selected.candidate_id,
            &resolved_model,
            &key.scope_snapshot(),
            &[WireApi::Responses],
        ) else {
            continue;
        };
        if route.account_id.is_none() {
            continue;
        }
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.service_tier = service_tier;
        let cooldown_context = CooldownContext {
            scope: &route.scope,
            allowed_protocols: &route.allowed_protocols,
        };
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
        let tool_use = with_forwarded_tool_diagnostics(&client_tool_use, &request_body);

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
                let state = apply_attempt_failure_cooldown(
                    &runtime,
                    &route.candidate_id,
                    &route.source_model,
                    &failure,
                    &HeaderMap::new(),
                    &cooldown_context,
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
                    tool_use.clone(),
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        let status = upstream.status();
        let response_headers = upstream.headers().clone();
        let bytes = match crate::runtime::collect_limited(upstream, endpoint.response_limit()).await
        {
            Ok(bytes) => bytes,
            Err(_) => {
                let failure = AttemptFailure::body();
                let state = apply_cooldown_for_model(
                    &runtime,
                    &route.candidate_id,
                    "*",
                    &route.source_model,
                    TRANSIENT_COOLDOWN_MS,
                    &cooldown_context,
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
                    tool_use.clone(),
                );
                apply_failure_state(&mut event, state);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                continue;
            }
        };
        if !status.is_success() {
            if !function_item_id_repair_attempted
                && responses_function_item_id_requires_fc_prefix(&bytes)
                && repair_call_prefixed_function_item_ids(&mut request)
            {
                function_item_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                tried.remove(&route.candidate_id);
                continue;
            }
            if !message_item_id_repair_attempted
                && responses_message_item_id_requires_msg_prefix(&bytes)
                && remove_item_prefixed_message_ids(&mut request)
            {
                message_item_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                tried.remove(&route.candidate_id);
                continue;
            }
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            last_preserved_upstream_error = preserved_upstream_error(&failure, &bytes);
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
                tool_use.clone(),
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
                        &cooldown_context,
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
            tool_use,
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
        if let Some((retry_at, reason)) = runtime.all_applicable_cooldown(
            &key,
            &resolved_model,
            &[WireApi::Responses],
            &account_only_exclusions,
            response_affinity_key.as_deref(),
            now_ms(),
        ) {
            return cooldown_error(
                retry_at,
                Some(&failure),
                reason == crate::scheduler::CooldownReason::RateLimit,
            );
        }
    }
    if let Some(preserved) = last_preserved_upstream_error.as_ref().filter(|preserved| {
        preserved.status == failure.status && preserved.category == failure.category
    }) {
        return api_error(preserved.status, &preserved.message, &preserved.code);
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
    let key = runtime
        .authenticate(headers.get(AUTHORIZATION))
        .or_else(|| {
            (wire_api == WireApi::Messages)
                .then(|| headers.get("x-api-key"))
                .flatten()
                .and_then(|value| value.to_str().ok())
                .and_then(|secret| runtime.authenticate_secret(secret))
        });
    let Some(key) = key else {
        return unauthorized();
    };
    let client_wire_api = match wire_api {
        WireApi::Responses => ClientWireApi::Responses,
        WireApi::ChatCompletions => ClientWireApi::ChatCompletions,
        WireApi::Messages => ClientWireApi::Messages,
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
    if wire_api == WireApi::ChatCompletions && chat_request_uses_tools(&request) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "tool use is not supported through Chat Completions; use Responses or Messages",
            "tool_use_not_supported",
        );
    }
    if wire_api == WireApi::ChatCompletions && !chat_request_is_text_or_image_only(&request) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "Chat Completions supports text and image content only",
            "chat_feature_not_supported",
        );
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
            "model is not available in this managed pool",
            "model_not_found",
        );
    };
    let responses_lite = (wire_api == WireApi::Responses)
        .then(|| {
            headers
                .get(CODEX_RESPONSES_LITE_HEADER)
                .cloned()
                .or_else(|| {
                    runtime
                        .codex_model_uses_responses_lite(&resolved_model)
                        .then(|| HeaderValue::from_static("true"))
                })
        })
        .flatten();
    let response_affinity_key = (wire_api == WireApi::Responses)
        .then(|| {
            runtime
                .response_affinity_key(request.get("previous_response_id").and_then(Value::as_str))
        })
        .flatten();
    let request_id = request_id();
    let forwarded_headers = match wire_api {
        WireApi::Messages => forwarded_messages_headers(&headers),
        WireApi::Responses | WireApi::ChatCompletions => {
            forwarded_codex_headers(&headers, &request_id)
        }
    };
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
    let service_tier = if wire_api != WireApi::Messages {
        request_service_tier(&request)
    } else {
        DefaultServiceTier::Standard
    };
    let client_tool_use = tool_use_diagnostics(&request);
    let mut tried = HashSet::new();
    let mut attempt = attempt_offset;
    let mut attempts_this_run = 0_usize;
    let mut owner_recovery_confirmed = false;
    let mut confirmed_response_missing = false;
    let mut encrypted_content_recovered = false;
    let mut native_replay_attempted = false;
    let mut function_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
    let mut last_failure = None;
    let mut last_adapter_error: Option<AdapterError> = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;
    let has_previous_response_id = wire_api == WireApi::Responses
        && request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
    let prompt_affinity_key = (wire_api != WireApi::Messages)
        .then(|| {
            runtime.prompt_affinity_key(
                &key.id,
                &resolved_model,
                request.get("prompt_cache_key").and_then(Value::as_str),
            )
        })
        .flatten();

    while attempts_this_run
        < retry_candidate_limit(runtime.max_retry_candidates(), owner_recovery_confirmed)
            + usize::from(encrypted_content_recovered)
            + usize::from(native_replay_attempted)
    {
        let selected = runtime
            .select_and_reserve(
                &key,
                &resolved_model,
                candidate_protocols(wire_api),
                &tried,
                (
                    response_affinity_key.as_deref(),
                    prompt_affinity_key.as_deref(),
                ),
                now_ms(),
            )
            .await;
        let Some((selected, lease)) = selected else {
            if attempt == 0 {
                if let Some((retry_at, reason)) = runtime.all_applicable_cooldown(
                    &key,
                    &resolved_model,
                    candidate_protocols(wire_api),
                    &tried,
                    response_affinity_key.as_deref(),
                    now_ms(),
                ) {
                    return cooldown_error(
                        retry_at,
                        None,
                        reason == crate::scheduler::CooldownReason::RateLimit,
                    );
                }
            }
            if last_failure.is_none() {
                if let Some(error) = last_adapter_error {
                    return adapter_error_response(error);
                }
            }
            break;
        };
        tried.insert(selected.candidate_id.clone());
        let response_affinity_hit = selected.response_affinity_hit;
        let allowed_protocols = candidate_protocols(wire_api);
        let Some(mut route) = runtime.executor_route(
            &selected.candidate_id,
            &resolved_model,
            &key.scope_snapshot(),
            allowed_protocols,
        ) else {
            continue;
        };
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.service_tier = service_tier;
        let cooldown_context = CooldownContext {
            scope: &route.scope,
            allowed_protocols: &route.allowed_protocols,
        };
        let source_model = route.source_model.clone();
        debug_assert_eq!(wire_api, route.wire_api);
        let account_route = route.account_id.is_some();
        let previous = if route.adapter.uses_local_continuation_state() {
            match request
                .get("previous_response_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                Some(response_id) => match runtime.load_messages_bridge_state(
                    &key.id,
                    response_id,
                    &route.candidate_id,
                    now_ms(),
                ) {
                    Ok(previous) => Some(previous),
                    Err(error) => return adapter_error_response(error),
                },
                None => None,
            }
        } else {
            None
        };
        let mut adapter_request = match route.adapter.prepare_request(AdapterRequestContext {
            client_wire_api: wire_api,
            request: &request,
            model: &source_model,
            stream,
            reasoning_mode: route.reasoning_mode,
            previous,
            response_scope: &route.candidate_id,
        }) {
            Ok(request) => request,
            Err(error) if error.is_route_incompatible() => {
                last_adapter_error = Some(error);
                continue;
            }
            Err(error) => return adapter_error_response(error),
        };
        if account_route {
            let Some(upstream_body) = adapter_request.native_upstream_body_mut() else {
                return adapter_error_response(AdapterError::unsupported_binding());
            };
            let Value::Object(object) = upstream_body else {
                unreachable!("request object was validated before execution")
            };
            normalize_account_request(object, responses_lite.is_some());
        }
        let adapter_is_passthrough = adapter_request.is_passthrough();
        let request_body = match serde_json::to_vec(adapter_request.upstream_body()) {
            Ok(body) => body,
            Err(_) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "request body could not be serialized",
                    "invalid_request",
                )
            }
        };
        let tool_use = with_forwarded_tool_diagnostics(&client_tool_use, &request_body);

        let upstream_stream = stream;
        attempt = attempt.saturating_add(1);
        attempts_this_run = attempts_this_run.saturating_add(1);
        let started = Instant::now();
        let client = runtime.request_client(&route.candidate_id, upstream_stream);
        let mut upstream_headers = if adapter_request.requires_bridge_headers() {
            forwarded_bridge_messages_headers(&forwarded_headers)
        } else {
            forwarded_headers.clone()
        };
        for (name, value) in &route.upstream_headers {
            upstream_headers.insert(name.clone(), value.clone());
        }
        let mut upstream_request = client
            .post(route.upstream_url.clone())
            .header(CONTENT_TYPE, "application/json")
            .headers(upstream_headers);
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
                let state = apply_attempt_failure_cooldown(
                    &runtime,
                    &route.candidate_id,
                    &source_model,
                    &failure,
                    &HeaderMap::new(),
                    &cooldown_context,
                    route.half_open_probe,
                );
                if failure_requires_independent_source_endpoint(failure.status, failure.category) {
                    runtime.exclude_same_source_endpoint(&route.candidate_id, &mut tried);
                }
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
                    tool_use.clone(),
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
                tool_use.clone(),
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
                        &cooldown_context,
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                    if failure_requires_independent_source_endpoint(
                        failure.status,
                        failure.category,
                    ) {
                        runtime.exclude_same_source_endpoint(&route.candidate_id, &mut tried);
                    }
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    continue;
                }
                Err(error) => return upstream_body_error_response(&runtime, event, started, error),
            };
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && !function_item_id_repair_attempted
                && responses_function_item_id_requires_fc_prefix(&bytes)
                && repair_call_prefixed_function_item_ids(&mut request)
            {
                function_item_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                attempts_this_run = attempts_this_run.saturating_sub(1);
                tried.remove(&route.candidate_id);
                continue;
            }
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && !message_item_id_repair_attempted
                && responses_message_item_id_requires_msg_prefix(&bytes)
                && remove_item_prefixed_message_ids(&mut request)
            {
                message_item_id_repair_attempted = true;
                attempt = attempt.saturating_sub(1);
                attempts_this_run = attempts_this_run.saturating_sub(1);
                tried.remove(&route.candidate_id);
                continue;
            }
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            last_preserved_upstream_error = preserved_upstream_error(&failure, &bytes);
            event.error_category = Some(failure.category.to_string());
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && has_previous_response_id
                && !native_replay_attempted
                && (previous_response_requires_websocket(&bytes)
                    || (status == StatusCode::BAD_REQUEST
                        && contains_tool_call_output(&request)
                        && responses_function_call_output_has_invalid_call_id(&bytes)))
            {
                let previous_response_id = request
                    .get("previous_response_id")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty());
                if let Some(previous_response_id) = previous_response_id {
                    if let Some(replay) = runtime.load_native_responses_replay(
                        &key.id,
                        previous_response_id,
                        &route.candidate_id,
                        now_ms(),
                    ) {
                        request = match replay.replay_request(&request, &source_model, stream) {
                            Ok(request) => request,
                            Err(error) => return adapter_error_response(error),
                        };
                        native_replay_attempted = true;
                        tried.remove(&route.candidate_id);
                        emit_usage(&runtime, event);
                        last_failure = Some(failure);
                        continue;
                    }
                }
            }
            if wire_api == WireApi::Responses
                && failure.category == "upstream_encrypted_content_invalid"
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
                        &cooldown_context,
                        route.half_open_probe,
                    );
                    apply_failure_state(&mut event, state);
                    if failure_requires_independent_source_endpoint(
                        failure.status,
                        failure.category,
                    ) {
                        runtime.exclude_same_source_endpoint(&route.candidate_id, &mut tried);
                    }
                }
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                if affinity_miss && response_missing && response_affinity_hit {
                    break;
                }
                continue;
            }
            if !adapter_is_passthrough {
                event.error_category = Some("adapter_upstream_error".to_string());
                emit_usage(&runtime, event);
                return api_error(
                    failure.status,
                    "upstream source rejected the translated request",
                    "adapter_upstream_error",
                );
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
                    let failure = AttemptFailure::body();
                    let state = apply_cooldown_for_model(
                        &runtime,
                        &route.candidate_id,
                        "*",
                        &source_model,
                        TRANSIENT_COOLDOWN_MS,
                        &cooldown_context,
                        route.half_open_probe,
                    );
                    if failure_requires_independent_source_endpoint(
                        failure.status,
                        failure.category,
                    ) {
                        runtime.exclude_same_source_endpoint(&route.candidate_id, &mut tried);
                    }
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
                        tool_use.clone(),
                    );
                    apply_failure_state(&mut event, state);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    continue;
                }
            };
            let bytes = if account_route {
                match completed_account_response(&bytes) {
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
                                    &cooldown_context,
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
                            tool_use.clone(),
                        );
                        if wire_api == WireApi::Responses
                            && failure.category == "upstream_encrypted_content_invalid"
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
                            if let Some(preserved) =
                                last_preserved_upstream_error.as_ref().filter(|preserved| {
                                    preserved.status == failure.status
                                        && preserved.category == failure.category
                                })
                            {
                                return api_error(
                                    preserved.status,
                                    &preserved.message,
                                    &preserved.code,
                                );
                            }
                            return api_error(failure.status, failure.message, failure.category);
                        }
                        last_failure = Some(failure);
                        continue;
                    }
                }
            } else {
                bytes
            };
            let bridge_response = match adapter_request.translate_response_bytes(&bytes) {
                Ok(response) => response,
                Err(error) => {
                    let mut event = usage_event(
                        &request_id,
                        attempt,
                        &key.id,
                        &route,
                        &requested_model,
                        false,
                        StatusCode::BAD_GATEWAY.as_u16(),
                        Some(error.code().to_string()),
                        started.elapsed().as_millis() as u64,
                        tool_use.clone(),
                    );
                    event.error_category = Some(error.code().to_string());
                    emit_usage(&runtime, event);
                    drop(lease);
                    return adapter_error_response(error);
                }
            };
            let bytes = if let Some(response) = bridge_response.as_ref() {
                match serde_json::to_vec(&response.response_body) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        let error = AdapterError::upstream_response_invalid();
                        let mut event = usage_event(
                            &request_id,
                            attempt,
                            &key.id,
                            &route,
                            &requested_model,
                            false,
                            StatusCode::BAD_GATEWAY.as_u16(),
                            Some(error.code().to_string()),
                            started.elapsed().as_millis() as u64,
                            tool_use.clone(),
                        );
                        event.error_category = Some(error.code().to_string());
                        emit_usage(&runtime, event);
                        drop(lease);
                        return adapter_error_response(error);
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
                tool_use.clone(),
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
            if let Some(bridge_response) = bridge_response.as_ref() {
                runtime.save_messages_bridge_response(
                    &key.id,
                    &route.candidate_id,
                    bridge_response,
                    now_ms(),
                );
            }
            if wire_api == WireApi::Responses && adapter_is_passthrough {
                if let Ok(upstream) = serde_json::from_slice::<Value>(&bytes) {
                    if let Some((response_id, replay)) =
                        crate::NativeResponsesReplayState::from_response(
                            &request,
                            &source_model,
                            &upstream,
                        )
                    {
                        runtime.save_native_responses_replay(
                            &key.id,
                            &route.candidate_id,
                            &response_id,
                            replay,
                            now_ms(),
                        );
                    }
                }
            }
            if wire_api == WireApi::Responses {
                let completed_response_id = response_id_from_bytes(&bytes);
                runtime.bind_response_affinity(
                    completed_response_id.as_deref(),
                    &route.candidate_id,
                    now_ms(),
                );
            }
            if account_route || !adapter_is_passthrough {
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
                let completion_uses_response_affinity = wire_api == WireApi::Responses;
                let completion_bridge_state = (!adapter_is_passthrough)
                    .then(|| Arc::new(Mutex::new(None::<MessagesBridgeResponse>)));
                let completion_bridge_state_for_callback = completion_bridge_state.clone();
                let completion_native_response = (wire_api == WireApi::Responses
                    && adapter_is_passthrough)
                    .then(|| Arc::new(Mutex::new(None::<Value>)));
                let completion_native_response_for_callback = completion_native_response.clone();
                let completion_native_template = request.clone();
                let completion_local_key = key.id.clone();
                let completion_scope = route.scope.clone();
                let completion_allowed_protocols = route.allowed_protocols.clone();
                let completion: CompletionCallback = Arc::new(move |event, response_id, hint| {
                    lease.release();
                    let response_delivered = event.success
                        || event.error_category.as_deref() == Some("response_incomplete");
                    if response_delivered {
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
                        if completion_uses_response_affinity {
                            completion_runtime.bind_response_affinity(
                                response_id,
                                &completion_source,
                                now_ms(),
                            );
                        }
                        if let Some(shared) = completion_bridge_state_for_callback.as_ref() {
                            if let Some(response) = shared
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .take()
                            {
                                completion_runtime.save_messages_bridge_response(
                                    &completion_local_key,
                                    &completion_source,
                                    &response,
                                    now_ms(),
                                );
                            }
                        }
                        if let Some(shared) = completion_native_response_for_callback.as_ref() {
                            if let Some(response) = shared
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .take()
                            {
                                if let Some((response_id, replay)) =
                                    crate::NativeResponsesReplayState::from_response(
                                        &completion_native_template,
                                        &completion_model,
                                        &response,
                                    )
                                {
                                    completion_runtime.save_native_responses_replay(
                                        &completion_local_key,
                                        &completion_source,
                                        &response_id,
                                        replay,
                                        now_ms(),
                                    );
                                }
                            }
                        }
                    } else if let Some(category) = event
                        .error_category
                        .as_deref()
                        .filter(|category| failure_category_requires_cooldown(category))
                    {
                        let status = StatusCode::from_u16(event.http_status)
                            .unwrap_or(StatusCode::BAD_GATEWAY);
                        let cooldown_context = CooldownContext {
                            scope: &completion_scope,
                            allowed_protocols: &completion_allowed_protocols,
                        };
                        let state = apply_failure_cooldown_with_hint(
                            &completion_runtime,
                            &completion_source,
                            &completion_model,
                            status,
                            category,
                            &completion_headers,
                            hint,
                            &cooldown_context,
                            completion_half_open_probe,
                        );
                        apply_failure_state(event, state);
                    }
                });
                let combined: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> =
                    if let Some(bridge) = adapter_request.into_stream_bridge() {
                        let completed = completion_bridge_state
                            .clone()
                            .expect("bridge state is configured for bridge routes");
                        Box::pin(bridge_messages_stream(first, remaining, bridge, completed))
                    } else {
                        Box::pin(
                            stream::once(async move { Ok::<_, reqwest::Error>(first) })
                                .chain(remaining),
                        )
                    };
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
                        tool_use.clone(),
                    ),
                    started,
                    completion,
                    completion_native_response,
                );
                return proxy_sse_response(status, &headers, Body::from_stream(usage_stream));
            }
            Err(bootstrap_failure) => {
                let failure = bootstrap_failure.failure;
                last_preserved_upstream_error = bootstrap_failure.preserved;
                let state = failure_category_requires_cooldown(failure.category).then(|| {
                    apply_attempt_failure_cooldown(
                        &runtime,
                        &route.candidate_id,
                        &source_model,
                        &failure,
                        &response_headers,
                        &cooldown_context,
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
                    tool_use.clone(),
                );
                if wire_api == WireApi::Responses
                    && failure.category == "upstream_encrypted_content_invalid"
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
                    if let Some(preserved) =
                        last_preserved_upstream_error.as_ref().filter(|preserved| {
                            preserved.status == failure.status
                                && preserved.category == failure.category
                        })
                    {
                        return api_error(preserved.status, &preserved.message, &preserved.code);
                    }
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

    if last_failure.is_none() {
        if let Some(error) = last_adapter_error {
            return adapter_error_response(error);
        }
    }
    let failure = last_failure.unwrap_or_else(AttemptFailure::no_candidate);
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some((retry_at, reason)) = runtime.all_applicable_cooldown(
            &key,
            &resolved_model,
            candidate_protocols(wire_api),
            &HashSet::new(),
            response_affinity_key.as_deref(),
            now_ms(),
        ) {
            return cooldown_error(
                retry_at,
                Some(&failure),
                reason == crate::scheduler::CooldownReason::RateLimit,
            );
        }
    }
    if let Some(preserved) = last_preserved_upstream_error.as_ref().filter(|preserved| {
        preserved.status == failure.status && preserved.category == failure.category
    }) {
        return api_error(preserved.status, &preserved.message, &preserved.code);
    }
    api_error(failure.status, failure.message, failure.category)
}

fn adapter_error_response(error: AdapterError) -> Response<Body> {
    let status = if error.is_upstream_failure() {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::BAD_REQUEST
    };
    api_error(status, error.message(), error.code())
}
