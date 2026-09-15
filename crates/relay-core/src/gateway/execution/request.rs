use super::super::errors::{
    api_error, api_error_with_origin, api_error_with_origin_and_category,
    apply_attempt_failure_cooldown, apply_cooldown_for_model, apply_failure_cooldown_with_body,
    apply_failure_state, cooldown_error, failure_category_is_request_terminal,
    failure_category_requires_cooldown, preserved_upstream_error, previous_response_not_found,
    previous_response_requires_websocket, prompt_cache_write_rejected,
    recoverable_response_affinity_miss, recoverable_response_model_switch,
    responses_call_id_is_missing, responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_call_output_has_invalid_call_id,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    responses_tool_call_is_missing_output, responses_tool_call_is_missing_output_message,
    retry_candidate_limit, retryable_failure, retryable_status, zenith_gateway_invalid_request,
    AttemptFailure, CooldownContext, PreservedUpstreamError, TRANSIENT_COOLDOWN_MS,
};
use super::super::now_ms;
#[cfg(test)]
use super::super::request::requested_reasoning_effort;
use super::super::request::{
    apply_codex_routing_hint, candidate_protocols, codex_client_version, contains_tool_call_output,
    drop_unpaired_responses_tool_calls, forwarded_bridge_gemini_headers,
    forwarded_bridge_messages_headers, normalize_account_request, normalize_responses_lite_request,
    repair_legacy_responses_call_ids, response_tool_call_ids,
    responses_lite_parallel_tool_calls_valid, tool_call_output_ids, tool_use_diagnostics,
    try_recover_encrypted_content, with_forwarded_tool_diagnostics, ServiceTierPolicy,
    CODEX_RESPONSES_LITE_HEADER,
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
use super::finish_request_failure;
use super::{wait_for_candidate_retry, CandidateRetryContext};
use crate::protocol::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids, AdapterError, AdapterRequestContext, AdapterResponse,
    PreparedAdapterRequest,
};
use crate::runtime::AuthenticatedKey;
use crate::usage::ReasoningEffortDiagnostics;
use crate::{Error, GatewayRuntime, SourceAdapter, WireApi};
use axum::body::Body;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

pub(super) struct RequestExecution {
    pub(super) runtime: Arc<GatewayRuntime>,
    pub(super) key: AuthenticatedKey,
    pub(super) request: Value,
    pub(super) service_tier_policy: ServiceTierPolicy,
    pub(super) requested_model: String,
    pub(super) resolved_model: String,
    pub(super) stream: bool,
    pub(super) request_id: String,
    pub(super) forwarded_headers: HeaderMap,
    pub(super) client_context_id: Option<String>,
    pub(super) response_affinity_key: Option<String>,
    pub(super) requires_affinity_owner: bool,
    pub(super) wait_for_candidate_availability: bool,
    pub(super) wire_api: WireApi,
    pub(super) responses_lite: Option<HeaderValue>,
    pub(super) allow_previous_response_reset: bool,
    pub(super) attempt_offset: u16,
}

/// Keeps adapter translation and JSON serialization at the protocol boundary.
/// Native responses preserve their upstream bytes, while bridged responses
/// return both the client representation and continuation state.
fn translate_completed_response(
    adapter_request: PreparedAdapterRequest,
    upstream_bytes: Vec<u8>,
) -> Result<(Vec<u8>, Option<AdapterResponse>), AdapterError> {
    let bridge_response = adapter_request.translate_response_bytes(&upstream_bytes)?;
    let response_bytes = bridge_response
        .as_ref()
        .map(|response| {
            serde_json::to_vec(response.response_body())
                .map_err(|_| AdapterError::upstream_response_invalid())
        })
        .transpose()?
        .unwrap_or(upstream_bytes);
    Ok((response_bytes, bridge_response))
}

pub(super) async fn execute_request(context: RequestExecution) -> Response<Body> {
    let RequestExecution {
        runtime,
        key,
        mut request,
        service_tier_policy,
        requested_model,
        resolved_model,
        stream,
        request_id,
        forwarded_headers,
        client_context_id,
        mut response_affinity_key,
        mut requires_affinity_owner,
        wait_for_candidate_availability,
        wire_api,
        responses_lite,
        allow_previous_response_reset,
        attempt_offset,
    } = context;
    let client_tool_use = tool_use_diagnostics(&request);
    let mut tried: HashSet<String> = Default::default();
    let mut attempt = attempt_offset;
    let mut attempts_this_run = 0_usize;
    let mut owner_recovery_confirmed = false;
    let mut confirmed_response_missing = false;
    let mut encrypted_content_recovered = false;
    let mut native_replay_attempted = false;
    let mut function_item_id_repair_attempted = false;
    let mut custom_tool_item_id_repair_attempted = false;
    let mut message_item_id_repair_attempted = false;
    let mut legacy_call_id_repair_attempted = false;
    let mut model_switch_reset_attempted = false;
    let mut stale_tool_history_recovered = false;
    let mut last_failure: Option<AttemptFailure> = None;
    let mut last_adapter_error: Option<AdapterError> = None;
    let mut last_preserved_upstream_error: Option<PreservedUpstreamError> = None;
    let mut last_failure_origin = crate::ErrorOrigin::Relay;
    let mut retry_deadline = (!runtime.chatgpt_retry_until_available()).then(|| {
        tokio::time::Instant::now() + Duration::from_millis(runtime.chatgpt_retry_window_ms())
    });
    let mut retry_wait_attempt = 0u32;
    let mut retry_window_expired = false;
    // Automatic Lite is safe only when every configured route in this key
    // scope is an official account with confirmed Lite support. Explicit
    // client Lite headers remain authoritative, but a mixed or partly unknown
    // pool must use full Responses so fallback preserves its tool/context
    // contract.
    let automatic_responses_lite = wire_api == WireApi::Responses
        && runtime.codex_model_responses_routes_all_support_lite(&key, &resolved_model);
    let mut has_unpaired_tool_output = {
        let outputs = tool_call_output_ids(&request);
        let calls = response_tool_call_ids(&request);
        outputs
            .iter()
            .any(|output| !calls.iter().any(|call| call == output))
    };
    let prompt_affinity_key = runtime.prompt_affinity_key(
        &key.id,
        &resolved_model,
        request.get("prompt_cache_key").and_then(Value::as_str),
        client_context_id.as_deref(),
    );
    let retry_context = CandidateRetryContext {
        runtime: &runtime,
        key: &key,
        resolved_model: &resolved_model,
        protocols: candidate_protocols(wire_api),
    };

    loop {
        // Recovery can deliberately remove an unusable opaque response id.
        // Derive continuation semantics from the request that will actually be
        // sent on this attempt, rather than from its original payload.
        let has_previous_response_id = request_has_previous_response_id(wire_api, &request);
        // The request remembers whether it is an eligible managed ChatGPT
        // request; the policy itself is intentionally live. In particular,
        // turning the control off must stop an already-waiting request on its
        // next bounded availability poll.
        let retry_until_available = runtime.chatgpt_retry_until_available();
        // An eligible request can outlive a toggle change. Dropping a finite
        // deadline here lets an operator turn persistent recovery on while it
        // is waiting instead of preserving the old bounded policy.
        if retry_until_available {
            retry_deadline = None;
        }
        let wait_for_candidate_availability =
            wait_for_candidate_availability && retry_until_available;
        let attempt_limit =
            retry_candidate_limit(runtime.max_retry_candidates(), owner_recovery_confirmed)
                + usize::from(encrypted_content_recovered)
                + usize::from(native_replay_attempted)
                + usize::from(model_switch_reset_attempted)
                + usize::from(stale_tool_history_recovered)
                + usize::from(legacy_call_id_repair_attempted);
        if attempts_this_run >= attempt_limit {
            if should_wait_for_candidate_availability(
                wait_for_candidate_availability,
                &last_failure,
                last_adapter_error.is_some(),
                has_previous_response_id,
            ) {
                attempts_this_run = 0;
                let backoff = retry_backoff(retry_wait_attempt.saturating_add(1));
                if !wait_for_candidate_retry(
                    &retry_context,
                    &mut tried,
                    response_affinity_key.as_deref(),
                    &mut retry_wait_attempt,
                    backoff,
                    retry_deadline,
                )
                .await
                {
                    retry_window_expired = true;
                    break;
                }
                continue;
            }
            break;
        }
        // Every pre-output retry (including SSE and transport failures) must
        // release a replayable request's optional tool affinity. Otherwise
        // selection stops at its already-tried owner despite healthy routes.
        // Opaque response references and unpaired tool outputs stay pinned.
        if !requires_affinity_owner
            && last_failure
                .as_ref()
                .is_some_and(|failure| failure_category_requires_cooldown(failure.category))
        {
            runtime.invalidate_response_affinity(response_affinity_key.as_deref());
            response_affinity_key = None;
        }
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
            if !requires_affinity_owner
                && runtime.release_unroutable_response_affinity(
                    &key,
                    &mut response_affinity_key,
                    &resolved_model,
                    candidate_protocols(wire_api),
                    now_ms(),
                )
            {
                continue;
            }
            // Native replay deliberately refuses to materialize a response
            // across model/protocol routes. When the bound owner itself no
            // longer supports this route, retaining its opaque response id
            // would therefore block all eligible new-model candidates before
            // an upstream request is even attempted. Start a fresh safe turn
            // instead, but never infer that from temporary availability.
            if wire_api == WireApi::Responses
                && allow_previous_response_reset
                && has_previous_response_id
                && requires_affinity_owner
                && !has_unpaired_tool_output
                && !model_switch_reset_attempted
                && response_affinity_key.as_deref().and_then(|affinity_key| {
                    runtime.response_affinity_owner_supports_model(
                        affinity_key,
                        &resolved_model,
                        candidate_protocols(wire_api),
                        now_ms(),
                    )
                }) == Some(false)
                && request
                    .as_object_mut()
                    .is_some_and(|object| object.remove("previous_response_id").is_some())
            {
                model_switch_reset_attempted = true;
                runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                response_affinity_key = None;
                requires_affinity_owner = false;
                continue;
            }
            // Pool membership can change between two Codex turns.  An opaque
            // previous_response_id remains pinned to its old owner, so normal
            // selection correctly declines to send it to a new provider.  If
            // Relay still has the bounded, owner-scoped native replay for
            // that turn, materialize it before trying the replacement pool.
            // This keeps the continuation safe while avoiding a permanent
            // no-candidate failure after an operator rotates API sources.
            if wire_api == WireApi::Responses
                && has_previous_response_id
                && requires_affinity_owner
                && !native_replay_attempted
            {
                match replay_native_affinity_continuation(
                    &runtime,
                    &key.id,
                    &mut request,
                    response_affinity_key.as_deref(),
                    &resolved_model,
                    stream,
                    &mut native_replay_attempted,
                ) {
                    Ok(true) => {
                        runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                        response_affinity_key = None;
                        requires_affinity_owner = false;
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => return adapter_error_response(error),
                }
            }
            // If the owner still matches the model but is no longer in this
            // key's pool scope, there is no safe upstream route for the
            // opaque response id. Reset it only after giving the bounded
            // native replay above a chance to preserve the conversation.
            if wire_api == WireApi::Responses
                && allow_previous_response_reset
                && has_previous_response_id
                && requires_affinity_owner
                && !has_unpaired_tool_output
                && !model_switch_reset_attempted
                && response_affinity_key.as_deref().and_then(|affinity_key| {
                    runtime.response_affinity_owner_supports_route(
                        &key,
                        affinity_key,
                        &resolved_model,
                        candidate_protocols(wire_api),
                        now_ms(),
                    )
                }) == Some(false)
                && request
                    .as_object_mut()
                    .is_some_and(|object| object.remove("previous_response_id").is_some())
            {
                model_switch_reset_attempted = true;
                runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                response_affinity_key = None;
                requires_affinity_owner = false;
                continue;
            }
            if should_wait_for_candidate_availability(
                wait_for_candidate_availability,
                &last_failure,
                last_adapter_error.is_some(),
                has_previous_response_id,
            ) {
                attempts_this_run = 0;
                let backoff = retry_backoff(retry_wait_attempt.saturating_add(1));
                if !wait_for_candidate_retry(
                    &retry_context,
                    &mut tried,
                    response_affinity_key.as_deref(),
                    &mut retry_wait_attempt,
                    backoff,
                    retry_deadline,
                )
                .await
                {
                    retry_window_expired = true;
                    break;
                }
                continue;
            }
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
        let selected_service_tier =
            runtime.model_service_tier_for_candidate(&route.candidate_id, &route.source_model);
        service_tier_policy.prepare_for_candidate(&mut request, selected_service_tier, wire_api);
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_id.clone();
        route.service_tier =
            service_tier_policy.effective_tier(&request, selected_service_tier, wire_api);
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
                    (automatic_responses_lite
                        && route.account_id.as_deref().is_some_and(|candidate_id| {
                            runtime
                                .codex_model_responses_lite_candidates(&resolved_model)
                                .iter()
                                .any(|id| id == candidate_id)
                        }))
                    .then(|| HeaderValue::from_static("true"))
                })
            })
            .flatten();
        if route_responses_lite.is_some() {
            let Some(object) = request.as_object_mut() else {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "request body must be a JSON object",
                    "invalid_request",
                );
            };
            if !responses_lite_parallel_tool_calls_valid(object) {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "responses Lite requires parallel_tool_calls to be a boolean",
                    "invalid_request",
                );
            }
            // Apply the shared Lite contract before adapter translation. This
            // keeps native, bridged, OAuth, and API-source routes identical.
            normalize_responses_lite_request(object);
        }
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
        if let Some(upstream_body) = adapter_request.native_upstream_body_mut() {
            let Value::Object(object) = upstream_body else {
                unreachable!("request object was validated before execution")
            };
            // Lite was normalized before adapter preparation. Account
            // normalization adds only the native ChatGPT fields here.
            if account_route {
                normalize_account_request(object, route_responses_lite.is_some());
            }
        } else if account_route {
            return adapter_error_response(AdapterError::unsupported_binding());
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
        if account_route && wire_api == WireApi::Responses {
            apply_codex_routing_hint(
                &mut upstream_headers,
                &route.source_model,
                route.service_tier,
            );
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
                codex_client_version(&forwarded_headers),
            )
            .await;
        let upstream = match upstream {
            Ok(upstream) => {
                route.account_token_generation = upstream.account_token_generation;
                upstream.response
            }
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
            if try_repair_legacy_responses_call_ids(
                &mut request,
                wire_api,
                adapter_is_passthrough,
                status.is_client_error() && responses_call_id_is_missing(&bytes),
                &mut legacy_call_id_repair_attempted,
                &mut attempt,
                &mut attempts_this_run,
                &mut tried,
                &route.candidate_id,
                &mut has_unpaired_tool_output,
                &mut requires_affinity_owner,
            ) {
                continue;
            }
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
            let cache_write_rejected = prompt_cache_write_rejected(&bytes);
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
                match replay_native_tool_continuation(
                    &runtime,
                    &key.id,
                    &mut request,
                    &route,
                    stream,
                    &mut native_replay_attempted,
                ) {
                    Ok(true) => {
                        tried.remove(&route.candidate_id);
                        emit_usage(&runtime, event);
                        last_failure = Some(failure);
                        last_failure_origin = selected_error_origin;
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => return adapter_error_response(error),
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
            if wire_api == WireApi::Responses
                && has_previous_response_id
                && responses_tool_call_is_missing_output(&bytes)
                && recover_stale_tool_history(&mut request, &mut stale_tool_history_recovered)
            {
                runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                response_affinity_key = None;
                requires_affinity_owner = false;
                tried.remove(&route.candidate_id);
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
                continue;
            }
            if wire_api == WireApi::Responses
                && allow_previous_response_reset
                && !model_switch_reset_attempted
                && recoverable_response_model_switch(
                    status,
                    failure.category,
                    has_previous_response_id,
                    has_unpaired_tool_output,
                    &bytes,
                )
                && request
                    .as_object_mut()
                    .is_some_and(|object| object.remove("previous_response_id").is_some())
            {
                model_switch_reset_attempted = true;
                runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                response_affinity_key = None;
                requires_affinity_owner = false;
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
            // A Responses continuation normally has to stay on the creator
            // of `previous_response_id`.  When that owner is temporarily
            // unavailable, use the durable native replay captured from the
            // successful turn before selecting another candidate.  This is
            // the safe hand-off path: the new candidate receives the
            // materialized conversation, never a foreign opaque response id.
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && has_previous_response_id
                && response_affinity_hit
                && requires_affinity_owner
                && !native_replay_attempted
                && retryable_failure(status, failure.category, has_previous_response_id)
            {
                match replay_native_tool_continuation(
                    &runtime,
                    &key.id,
                    &mut request,
                    &route,
                    stream,
                    &mut native_replay_attempted,
                ) {
                    Ok(true) => {
                        runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                        response_affinity_key = None;
                        requires_affinity_owner = false;
                        // Keep the failed owner in `tried`: replay has
                        // materialized the conversation specifically so the
                        // next attempt can be handed to a different slot.
                        emit_usage(&runtime, event);
                        last_failure = Some(failure);
                        last_failure_origin = selected_error_origin;
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => return adapter_error_response(error),
                }
            }
            if cache_write_rejected {
                runtime.invalidate_prompt_affinity(prompt_affinity_key.as_deref());
            }
            if affinity_miss
                || cache_write_rejected
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
                emit_usage(&runtime, event);
                if let Some(preserved) = last_preserved_upstream_error.as_ref() {
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
            let (bytes, bridge_response) =
                match translate_completed_response(adapter_request, bytes) {
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
                        return adapter_error_response_for_origin(error, selected_error_origin);
                    }
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
                    runtime.capture_native_responses_replay(
                        &key.id,
                        &route.candidate_id,
                        &request,
                        &source_model,
                        &upstream,
                        now_ms(),
                    );
                }
            }
            if wire_api == WireApi::Responses {
                if let Ok(response) = serde_json::from_slice::<Value>(&bytes) {
                    for call_id in response_tool_call_ids(&response) {
                        runtime.bind_tool_call_affinity(
                            &key.id,
                            &call_id,
                            &route.candidate_id,
                            now_ms(),
                        );
                    }
                }
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

        match bootstrap_stream(upstream).await {
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
                let missing_tool_output =
                    bootstrap_failure.preserved.as_ref().is_some_and(|error| {
                        responses_tool_call_is_missing_output_message(&error.message)
                    });
                let missing_call_id = bootstrap_failure.responses_call_id_is_missing;
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
                if try_repair_legacy_responses_call_ids(
                    &mut request,
                    wire_api,
                    adapter_is_passthrough,
                    missing_call_id,
                    &mut legacy_call_id_repair_attempted,
                    &mut attempt,
                    &mut attempts_this_run,
                    &mut tried,
                    &route.candidate_id,
                    &mut has_unpaired_tool_output,
                    &mut requires_affinity_owner,
                ) {
                    continue;
                }
                if wire_api == WireApi::Responses
                    && adapter_is_passthrough
                    && has_previous_response_id
                    && !native_replay_attempted
                    && contains_tool_call_output(&request)
                    && zenith_gateway_invalid_request
                {
                    match replay_native_tool_continuation(
                        &runtime,
                        &key.id,
                        &mut request,
                        &route,
                        stream,
                        &mut native_replay_attempted,
                    ) {
                        Ok(true) => {
                            tried.remove(&route.candidate_id);
                            emit_usage(&runtime, event);
                            last_failure = Some(failure);
                            last_failure_origin = selected_error_origin;
                            continue;
                        }
                        Ok(false) => {}
                        Err(error) => return adapter_error_response(error),
                    }
                }
                if wire_api == WireApi::Responses
                    && has_previous_response_id
                    && missing_tool_output
                    && recover_stale_tool_history(&mut request, &mut stale_tool_history_recovered)
                {
                    runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                    response_affinity_key = None;
                    requires_affinity_owner = false;
                    tried.remove(&route.candidate_id);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    last_failure_origin = selected_error_origin;
                    continue;
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
        && request_has_previous_response_id(wire_api, &request)
        && confirmed_response_missing
        && !contains_tool_call_output(&request)
    {
        let mut reset_request = request;
        if let Some(object) = reset_request.as_object_mut() {
            object.remove("previous_response_id");
            return Box::pin(execute_request(RequestExecution {
                runtime,
                key,
                request: reset_request,
                service_tier_policy,
                requested_model,
                resolved_model,
                stream,
                request_id,
                forwarded_headers,
                client_context_id,
                response_affinity_key: None,
                requires_affinity_owner: false,
                wait_for_candidate_availability,
                wire_api,
                responses_lite,
                allow_previous_response_reset: false,
                attempt_offset: attempt,
            }))
            .await;
        }
    }

    if last_failure.is_none() {
        if let Some(error) = last_adapter_error {
            return adapter_error_response(error);
        }
    }
    let failure = if retry_window_expired {
        AttemptFailure::classified_with_hint(
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
            Default::default(),
        )
    } else {
        last_failure.unwrap_or_else(AttemptFailure::no_candidate)
    };
    finish_request_failure(
        &runtime,
        &key,
        &resolved_model,
        candidate_protocols(wire_api),
        &HashSet::new(),
        response_affinity_key.as_deref(),
        failure,
        last_preserved_upstream_error.as_ref(),
        last_failure_origin,
        &request_id,
    )
}

pub(super) fn should_wait_for_candidate_availability(
    enabled: bool,
    last_failure: &Option<AttemptFailure>,
    has_adapter_error: bool,
    has_previous_response_id: bool,
) -> bool {
    enabled
        && !has_adapter_error
        && last_failure.as_ref().is_none_or(|failure| {
            retryable_failure(failure.status, failure.category, has_previous_response_id)
                && !matches!(
                    failure.category,
                    "upstream_unauthorized"
                        | "upstream_account_disabled"
                        | "upstream_usage_not_included"
                        | "upstream_region_unsupported"
                        | "upstream_model_not_found"
                        | "upstream_model_unsupported"
                        | "upstream_forbidden"
                        | "upstream_content_policy"
                        | "upstream_invalid_request"
                        | "upstream_candidate_rejected"
                )
        })
}

fn retry_backoff(attempt: u32) -> Duration {
    // Bounded deterministic jitter keeps simultaneous requests from waking in
    // lockstep without adding a random source to the request pipeline.
    let exponent = attempt.min(6);
    let base_ms = 100u64.saturating_mul(1u64 << exponent);
    let jitter_ms = u64::from((attempt.wrapping_mul(37)) % 100);
    Duration::from_millis((base_ms + jitter_ms).min(5_000))
}

fn recover_stale_tool_history(request: &mut Value, recovered: &mut bool) -> bool {
    if *recovered
        || !drop_unpaired_responses_tool_calls(request)
        || !request
            .as_object_mut()
            .is_some_and(|object| object.remove("previous_response_id").is_some())
    {
        return false;
    }
    *recovered = true;
    true
}

fn request_has_previous_response_id(wire_api: WireApi, request: &Value) -> bool {
    wire_api == WireApi::Responses
        && request
            .get("previous_response_id")
            .and_then(Value::as_str)
            .is_some_and(|value| !value.trim().is_empty())
}

/// Applies the one permitted repair for a strict upstream rejection, then
/// recomputes route-affinity state from the repaired request before retrying.
/// The same rejection can arrive as either a buffered error or a terminal
/// stream bootstrap failure, so both paths share this mutation.
#[allow(clippy::too_many_arguments)]
fn try_repair_legacy_responses_call_ids(
    request: &mut Value,
    wire_api: WireApi,
    adapter_is_passthrough: bool,
    upstream_rejected_missing_call_id: bool,
    repair_attempted: &mut bool,
    attempt: &mut u16,
    attempts_this_run: &mut usize,
    tried: &mut HashSet<String>,
    candidate_id: &str,
    has_unpaired_tool_output: &mut bool,
    requires_affinity_owner: &mut bool,
) -> bool {
    if wire_api != WireApi::Responses
        || !adapter_is_passthrough
        || *repair_attempted
        || !upstream_rejected_missing_call_id
        || !repair_legacy_responses_call_ids(request)
    {
        return false;
    }

    *repair_attempted = true;
    *attempt = attempt.saturating_sub(1);
    *attempts_this_run = attempts_this_run.saturating_sub(1);
    tried.remove(candidate_id);
    let output_ids = tool_call_output_ids(request);
    let call_ids = response_tool_call_ids(request);
    *has_unpaired_tool_output = output_ids
        .iter()
        .any(|output_id| !call_ids.iter().any(|call_id| call_id == output_id));
    *requires_affinity_owner =
        request_has_previous_response_id(wire_api, request) || *has_unpaired_tool_output;
    true
}

fn replay_native_tool_continuation(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    route: &crate::runtime::ExecutorRoute,
    stream: bool,
    attempted: &mut bool,
) -> Result<bool, AdapterError> {
    let Some(previous_response_id) = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    let Some(replay) = runtime.load_native_responses_replay(
        local_key_id,
        previous_response_id,
        &route.candidate_id,
        now_ms(),
    ) else {
        return Ok(false);
    };
    *request = replay.replay_request(request, &route.source_model, stream)?;
    *attempted = true;
    Ok(true)
}

fn replay_native_affinity_continuation(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    response_affinity_key: Option<&str>,
    model: &str,
    stream: bool,
    attempted: &mut bool,
) -> Result<bool, AdapterError> {
    let Some(previous_response_id) = request
        .get("previous_response_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(false);
    };
    let Some(candidate_id) =
        response_affinity_key.and_then(|key| runtime.response_affinity_candidate(key, now_ms()))
    else {
        return Ok(false);
    };
    let Some(replay) = runtime.load_native_responses_replay(
        local_key_id,
        previous_response_id,
        &candidate_id,
        now_ms(),
    ) else {
        return Ok(false);
    };
    *request = replay.replay_request(request, model, stream)?;
    *attempted = true;
    Ok(true)
}

fn adapter_error_response(error: AdapterError) -> Response<Body> {
    adapter_error_response_for_origin(error, crate::ErrorOrigin::Relay)
}

fn adapter_error_response_for_origin(
    error: AdapterError,
    origin: crate::ErrorOrigin,
) -> Response<Body> {
    let status = if error.is_upstream_failure() {
        StatusCode::BAD_GATEWAY
    } else {
        StatusCode::BAD_REQUEST
    };
    api_error_with_origin(status, error.message(), error.code(), origin, None)
}

#[cfg(test)]
mod tests {
    use super::{
        requested_reasoning_effort, retry_backoff, should_wait_for_candidate_availability,
    };
    use crate::gateway::errors::AttemptFailure;
    use crate::WireApi;
    use axum::http::StatusCode;
    use serde_json::json;
    use std::time::Duration;

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

    #[test]
    fn retry_wait_is_opt_in_and_only_accepts_transient_failures() {
        let transient = Some(AttemptFailure::classified_with_hint(
            StatusCode::SERVICE_UNAVAILABLE,
            "upstream_unavailable",
            Default::default(),
        ));
        let auth = Some(AttemptFailure::classified_with_hint(
            StatusCode::UNAUTHORIZED,
            "upstream_unauthorized",
            Default::default(),
        ));
        let rejected = Some(AttemptFailure::classified_with_hint(
            StatusCode::BAD_REQUEST,
            "upstream_candidate_rejected",
            Default::default(),
        ));

        assert!(!should_wait_for_candidate_availability(
            false, &transient, false, false
        ));
        assert!(should_wait_for_candidate_availability(
            true, &transient, false, false
        ));
        assert!(!should_wait_for_candidate_availability(
            true, &auth, false, false
        ));
        assert!(!should_wait_for_candidate_availability(
            true, &rejected, false, false
        ));
    }

    #[test]
    fn retry_backoff_is_exponential_and_bounded() {
        assert_eq!(retry_backoff(0), Duration::from_millis(100));
        assert!(retry_backoff(4) > retry_backoff(1));
        assert!(retry_backoff(100) <= Duration::from_secs(5));
    }
}
