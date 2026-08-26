use super::super::errors::{
    api_error, api_error_with_origin, api_error_with_origin_and_category,
    apply_attempt_failure_cooldown, apply_cooldown_for_model, apply_failure_cooldown_with_body,
    apply_failure_state, cooldown_error, failure_category_is_request_terminal,
    failure_category_requires_cooldown, preserved_upstream_error, previous_response_not_found,
    previous_response_requires_websocket, recoverable_response_affinity_miss,
    responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_call_output_has_invalid_call_id,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    retry_candidate_limit, retryable_failure, retryable_status, zenith_gateway_invalid_request,
    AttemptFailure, CooldownContext, PreservedUpstreamError, TRANSIENT_COOLDOWN_MS,
};
use super::super::now_ms;
#[cfg(test)]
use super::super::request::requested_reasoning_effort;
use super::super::request::{
    apply_default_service_tier_if_missing, candidate_protocols, contains_tool_call_output,
    forwarded_bridge_gemini_headers, forwarded_bridge_messages_headers, normalize_account_request,
    request_service_tier, responses_lite_parallel_tool_calls_valid, tool_use_diagnostics,
    try_recover_encrypted_content, with_forwarded_tool_diagnostics, CODEX_RESPONSES_LITE_HEADER,
};
use super::super::response::{
    completed_account_response, emit_usage, populate_tokens, proxy_error_response,
    proxy_json_response, proxy_response, response_id_from_bytes, route_error_origin,
    upstream_body_error_response, usage_event,
};
use super::super::streaming::{bootstrap_stream, StreamExecution};
use super::super::turn_state::{
    guard_account_request, relay_account_response_header, CODEX_TURN_STATE_HEADER,
};
use crate::protocol::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids, AdapterError, AdapterRequestContext,
};
use crate::runtime::{AuthenticatedKey, DefaultServiceTier};
use crate::usage::ReasoningEffortDiagnostics;
use crate::{Error, GatewayRuntime, SourceAdapter, WireApi};
use axum::body::Body;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

#[allow(clippy::too_many_arguments)]
pub(in crate::gateway::execution) async fn execute_request(
    runtime: Arc<GatewayRuntime>,
    key: AuthenticatedKey,
    mut request: Value,
    requested_model: String,
    resolved_model: String,
    stream: bool,
    request_id: String,
    forwarded_headers: HeaderMap,
    client_context_id: Option<String>,
    response_affinity_key: Option<String>,
    wire_api: WireApi,
    responses_lite: Option<HeaderValue>,
    allow_previous_response_reset: bool,
    attempt_offset: u16,
) -> Response<Body> {
    let client_supplied_service_tier = request.get("service_tier").is_some();
    let service_tier = if wire_api != WireApi::Messages {
        request_service_tier(&request)
    } else {
        DefaultServiceTier::Standard
    };
    let client_tool_use = tool_use_diagnostics(&request);
    let mut tried = Default::default();
    let mut attempt = attempt_offset;
    let mut attempts_this_run = 0_usize;
    let mut owner_recovery_confirmed = false;
    let mut confirmed_response_missing = false;
    let mut encrypted_content_recovered = false;
    let mut native_replay_attempted = false;
    let mut function_item_id_repair_attempted = false;
    let mut custom_tool_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
    let mut last_failure = None;
    let mut last_adapter_error: Option<AdapterError> = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;
    let mut last_failure_origin = crate::ErrorOrigin::Relay;
    let has_previous_response_id = wire_api == WireApi::Responses
        && request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
        client_context_id.as_deref(),
    );

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
            stream,
        ) else {
            continue;
        };
        if !client_supplied_service_tier {
            request
                .as_object_mut()
                .expect("request object was validated before routing")
                .remove("service_tier");
        }
        if wire_api != WireApi::Messages {
            apply_default_service_tier_if_missing(
                &mut request,
                runtime.model_service_tier_for_candidate(&route.candidate_id, &route.source_model),
            );
        }
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_id.clone();
        route.service_tier = service_tier;
        let selected_error_origin = route_error_origin(&route);
        let cooldown_context = CooldownContext {
            scope: &route.scope,
            allowed_protocols: &route.allowed_protocols,
        };
        let source_model = route.source_model.clone();
        debug_assert_eq!(wire_api, route.wire_api);
        let account_route = route.account_id.is_some();
        let route_responses_lite = (wire_api == WireApi::Responses)
            .then(|| {
                responses_lite.clone().or_else(|| {
                    route
                        .account_id
                        .as_deref()
                        .is_some_and(|candidate_id| {
                            runtime
                                .codex_model_responses_lite_candidates(&resolved_model)
                                .iter()
                                .any(|id| id == candidate_id)
                        })
                        .then(|| HeaderValue::from_static("true"))
                })
            })
            .flatten();
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
            cache_write_ttl: route.cache_write_ttl,
            previous,
            response_scope: &route.candidate_id,
            response_id_seed: &request_id,
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
            if route_responses_lite.is_some() && !responses_lite_parallel_tool_calls_valid(object) {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "responses Lite requires parallel_tool_calls to be a boolean",
                    "invalid_request",
                );
            }
            normalize_account_request(object, route_responses_lite.is_some());
        }
        let reasoning_effort = ReasoningEffortDiagnostics::from_bodies(
            &request,
            adapter_request.upstream_body(),
            wire_api,
        );
        let adapter_is_passthrough = adapter_request.is_passthrough();
        let Ok(request_body) = serde_json::to_vec(adapter_request.upstream_body()) else {
            return api_error(
                StatusCode::BAD_REQUEST,
                "request body could not be serialized",
                "invalid_request",
            );
        };
        let tool_use = with_forwarded_tool_diagnostics(&client_tool_use, &request_body);

        let upstream_stream = stream;
        attempt = attempt.saturating_add(1);
        attempts_this_run = attempts_this_run.saturating_add(1);
        let started = Instant::now();
        let client = runtime.request_client(&route.candidate_id, upstream_stream);
        let mut upstream_headers = if adapter_request.requires_bridge_headers() {
            match route.adapter {
                SourceAdapter::ResponsesToMessages => {
                    forwarded_bridge_messages_headers(&forwarded_headers)
                }
                SourceAdapter::ResponsesToGemini => {
                    forwarded_bridge_gemini_headers(&forwarded_headers)
                }
                SourceAdapter::Native => HeaderMap::new(),
            }
        } else {
            forwarded_headers.clone()
        };
        for (name, value) in &route.upstream_headers {
            upstream_headers.insert(name.clone(), value.clone());
        }
        if account_route && wire_api == WireApi::Responses && route.adapter.is_passthrough() {
            guard_account_request(
                &runtime,
                &key.id,
                &mut upstream_headers,
                route.account_id.as_deref().unwrap_or_default(),
                now_ms(),
            );
        } else {
            upstream_headers.remove(CODEX_TURN_STATE_HEADER);
        }
        let mut upstream_request = client
            .post(route.upstream_url.clone())
            .header(CONTENT_TYPE, "application/json")
            .headers(upstream_headers);
        if upstream_stream {
            upstream_request = upstream_request.header(ACCEPT, "text/event-stream");
        }
        if account_route {
            if let Some(value) = route_responses_lite.as_ref() {
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
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    Some(&reasoning_effort),
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
                last_failure_origin = selected_error_origin;
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
                Some(&reasoning_effort),
                &requested_model,
                false,
                status.as_u16(),
                None,
                started.elapsed().as_millis() as u64,
                tool_use.clone(),
            );
            let bytes = match crate::transport::collect_limited(
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
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    last_failure_origin = selected_error_origin;
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
                && !custom_tool_item_id_repair_attempted
                && responses_custom_tool_item_id_requires_ctc_prefix(&bytes)
                && repair_custom_tool_item_ids(&mut request)
            {
                custom_tool_item_id_repair_attempted = true;
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
                        && (responses_function_call_output_has_invalid_call_id(&bytes)
                            || zenith_gateway_invalid_request(&bytes))))
            {
                let replay = match replay_native_tool_continuation(
                    &runtime,
                    &key.id,
                    &request,
                    &source_model,
                    &route.candidate_id,
                    stream,
                ) {
                    Ok(replay) => replay,
                    Err(error) => return adapter_error_response(error),
                };
                if let Some(replay) = replay {
                    request = replay;
                    native_replay_attempted = true;
                    tried.remove(&route.candidate_id);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    last_failure_origin = selected_error_origin;
                    continue;
                }
            }
            if wire_api == WireApi::Responses
                && failure.category == "upstream_encrypted_content_invalid"
                && try_recover_encrypted_content(&mut request, &mut encrypted_content_recovered)
            {
                tried.remove(&route.candidate_id);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
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
                }
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
                if affinity_miss && response_missing && response_affinity_hit {
                    break;
                }
                continue;
            }
            if !adapter_is_passthrough {
                event.error_category = Some("adapter_upstream_error".to_string());
                emit_usage(&runtime, event);
                if let Some(preserved) = last_preserved_upstream_error.as_ref() {
                    return api_error_with_origin_and_category(
                        preserved.status,
                        &preserved.message,
                        &preserved.code,
                        "adapter_upstream_error",
                        crate::ErrorOrigin::Relay,
                        Some(&request_id),
                    );
                }
                return api_error_with_origin(
                    failure.status,
                    "upstream source rejected the translated request",
                    "adapter_upstream_error",
                    crate::ErrorOrigin::Relay,
                    Some(&request_id),
                );
            }
            populate_tokens(&mut event, &bytes);
            emit_usage(&runtime, event);
            let mut response = proxy_error_response(
                status,
                &response_headers,
                Body::from(bytes),
                selected_error_origin,
                failure.category,
                Some(&request_id),
            );
            if account_route && adapter_is_passthrough {
                if let Some(account_id) = route.account_id.as_deref() {
                    relay_account_response_header(
                        &runtime,
                        &key.id,
                        &forwarded_headers,
                        account_id,
                        &response_headers,
                        &mut response,
                        now_ms(),
                    );
                }
            }
            return response;
        }

        if !upstream_stream {
            let bytes = match crate::transport::collect_limited(
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
                    let mut event = usage_event(
                        &request_id,
                        attempt,
                        &key.id,
                        &route,
                        Some(&reasoning_effort),
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
                    last_failure_origin = selected_error_origin;
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
                            Some(&reasoning_effort),
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
                            last_failure_origin = selected_error_origin;
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
                                return api_error_with_origin_and_category(
                                    preserved.status,
                                    &preserved.message,
                                    &preserved.code,
                                    preserved.category,
                                    selected_error_origin,
                                    Some(&request_id),
                                );
                            }
                            return api_error_with_origin(
                                failure.status,
                                failure.message,
                                failure.category,
                                selected_error_origin,
                                Some(&request_id),
                            );
                        }
                        last_failure = Some(failure);
                        last_failure_origin = selected_error_origin;
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
                        Some(&reasoning_effort),
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
                match serde_json::to_vec(response.response_body()) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        let error = AdapterError::upstream_response_invalid();
                        let mut event = usage_event(
                            &request_id,
                            attempt,
                            &key.id,
                            &route,
                            Some(&reasoning_effort),
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
                Some(&reasoning_effort),
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
            if let Some((response_id, continuation)) = bridge_response
                .as_ref()
                .and_then(|response| response.continuation())
            {
                runtime.save_messages_bridge_response(
                    &key.id,
                    &route.candidate_id,
                    &crate::MessagesBridgeResponse {
                        response_body: bridge_response
                            .as_ref()
                            .expect("continuation response is present")
                            .response_body()
                            .clone(),
                        response_id: response_id.to_string(),
                        continuation: continuation.clone(),
                    },
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
                let mut response =
                    proxy_json_response(status, &response_headers, Body::from(bytes));
                if account_route && adapter_is_passthrough {
                    if let Some(account_id) = route.account_id.as_deref() {
                        relay_account_response_header(
                            &runtime,
                            &key.id,
                            &forwarded_headers,
                            account_id,
                            &response_headers,
                            &mut response,
                            now_ms(),
                        );
                    }
                }
                return response;
            }
            return proxy_response(status, &response_headers, Body::from(bytes));
        }

        let wait_for_native_replay_error = wire_api == WireApi::Responses
            && adapter_is_passthrough
            && has_previous_response_id
            && !native_replay_attempted
            && contains_tool_call_output(&request);
        match bootstrap_stream(upstream, wait_for_native_replay_error).await {
            Ok((headers, first, remaining)) => {
                let account_id = route.account_id.clone();
                let mut response = StreamExecution {
                    runtime: runtime.clone(),
                    route,
                    lease,
                    adapter_request,
                    request,
                    request_id,
                    local_key_id: key.id.clone(),
                    requested_model,
                    source_model,
                    prompt_affinity_key,
                    wire_api,
                    reasoning_effort,
                    tool_use,
                    attempt,
                    started,
                }
                .into_response(status, headers.clone(), first, remaining);
                if let Some(account_id) = account_id.as_deref() {
                    relay_account_response_header(
                        &runtime,
                        &key.id,
                        &forwarded_headers,
                        account_id,
                        &headers,
                        &mut response,
                        now_ms(),
                    );
                }
                return response;
            }
            Err(bootstrap_failure) => {
                let zenith_gateway_invalid_request =
                    bootstrap_failure.zenith_gateway_invalid_request;
                let failure = bootstrap_failure.failure;
                last_preserved_upstream_error = bootstrap_failure.preserved;
                let mut event = usage_event(
                    &request_id,
                    attempt,
                    &key.id,
                    &route,
                    Some(&reasoning_effort),
                    &requested_model,
                    false,
                    failure.status.as_u16(),
                    Some(failure.category.to_string()),
                    started.elapsed().as_millis() as u64,
                    tool_use.clone(),
                );
                if wire_api == WireApi::Responses
                    && adapter_is_passthrough
                    && has_previous_response_id
                    && !native_replay_attempted
                    && contains_tool_call_output(&request)
                    && zenith_gateway_invalid_request
                {
                    let replay = match replay_native_tool_continuation(
                        &runtime,
                        &key.id,
                        &request,
                        &source_model,
                        &route.candidate_id,
                        stream,
                    ) {
                        Ok(replay) => replay,
                        Err(error) => return adapter_error_response(error),
                    };
                    if let Some(replay) = replay {
                        request = replay;
                        native_replay_attempted = true;
                        tried.remove(&route.candidate_id);
                        emit_usage(&runtime, event);
                        last_failure = Some(failure);
                        last_failure_origin = selected_error_origin;
                        continue;
                    }
                }
                if wire_api == WireApi::Responses
                    && failure.category == "upstream_encrypted_content_invalid"
                    && try_recover_encrypted_content(&mut request, &mut encrypted_content_recovered)
                {
                    tried.remove(&route.candidate_id);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    last_failure_origin = selected_error_origin;
                    continue;
                }
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
                        return api_error_with_origin_and_category(
                            preserved.status,
                            &preserved.message,
                            &preserved.code,
                            preserved.category,
                            selected_error_origin,
                            Some(&request_id),
                        );
                    }
                    return api_error_with_origin(
                        failure.status,
                        failure.message,
                        failure.category,
                        selected_error_origin,
                        Some(&request_id),
                    );
                }
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
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
                client_context_id,
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
        return api_error_with_origin_and_category(
            preserved.status,
            &preserved.message,
            &preserved.code,
            preserved.category,
            last_failure_origin,
            Some(&request_id),
        );
    }
    api_error_with_origin(
        failure.status,
        failure.message,
        failure.category,
        last_failure_origin,
        Some(&request_id),
    )
}

fn replay_native_tool_continuation(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &Value,
    source_model: &str,
    candidate_id: &str,
    stream: bool,
) -> Result<Option<Value>, AdapterError> {
    let Some(previous_response_id) = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    let Some(replay) = runtime.load_native_responses_replay(
        local_key_id,
        previous_response_id,
        candidate_id,
        now_ms(),
    ) else {
        return Ok(None);
    };
    replay
        .replay_request(request, source_model, stream)
        .map(Some)
}

fn adapter_error_response(error: AdapterError) -> Response<Body> {
    let status = if error.is_upstream_failure() {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::BAD_REQUEST
    };
    api_error(status, error.message(), error.code())
}

#[cfg(test)]
mod tests {
    use super::requested_reasoning_effort;
    use crate::WireApi;
    use serde_json::json;

    #[test]
    fn requested_reasoning_effort_uses_only_the_matching_client_contract() {
        let responses = json!({"reasoning": {"effort": " High "}});
        let chat = json!({"reasoning_effort": " Low "});

        assert_eq!(
            requested_reasoning_effort(&responses, WireApi::Responses),
            Some("high".to_string())
        );
        assert_eq!(
            requested_reasoning_effort(&chat, WireApi::ChatCompletions),
            Some("low".to_string())
        );
        assert_eq!(
            requested_reasoning_effort(&responses, WireApi::Messages),
            None
        );
        assert_eq!(
            requested_reasoning_effort(
                &json!({"reasoning": {"effort": "none"}}),
                WireApi::Responses
            ),
            None
        );
    }
}
