use super::super::continuation::{
    drop_materialized_previous_response_id,
    recover_stale_tool_history as replay_and_prune_stale_tool_history,
    RESPONSE_CONTINUATION_UNAVAILABLE_CODE, RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
};
use super::super::errors::{
    api_error, apply_failure_state, cooldown_error, current_failure_state,
    failure_category_is_request_terminal, failure_category_requires_cooldown,
    preserved_upstream_error, previous_response_not_found, previous_response_requires_websocket,
    prompt_cache_write_rejected, recoverable_response_affinity_miss,
    recoverable_response_model_switch, responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_call_output_has_invalid_call_id,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    responses_tool_call_is_missing_output, responses_tool_call_is_missing_output_message,
    responses_tool_call_links_rejected, retryable_failure, retryable_status,
    settle_attempt_failure, settle_status_failure, zenith_gateway_invalid_request, AttemptFailure,
    PreservedUpstreamError,
};
use super::super::now_ms;
#[cfg(test)]
use super::super::request::requested_reasoning_effort;
use super::super::request::{
    apply_codex_routing_hint, candidate_protocols, codex_client_version, contains_tool_call_output,
    forwarded_bridge_gemini_headers, forwarded_bridge_messages_headers,
    is_deferred_tool_search_compatibility_error, normalize_account_request,
    normalize_responses_lite_request, repair_legacy_responses_call_ids, response_tool_call_ids,
    responses_lite_parallel_tool_calls_valid, unpaired_tool_output_ids, RequestToolPolicy,
    ServiceTierPolicy, CODEX_RESPONSES_LITE_HEADER,
};
use super::super::response::{
    completed_upstream_response, emit_usage, populate_tokens, proxy_error_response,
    proxy_json_response, proxy_response, proxy_sse_response, response_id_from_bytes,
    route_error_origin, upstream_body_error_response, usage_event,
};
use super::super::streaming::{bootstrap_stream, StreamExecution};
use super::super::turn_state::{
    relay_account_response_header, request_scope, CODEX_TURN_STATE_HEADER,
};
use super::{attempt_error_response, finish_request_failure};
use super::{wait_for_candidate_retry, wait_for_recovery, CandidateRetryContext};
use crate::error_codes;
use crate::protocol::{
    remove_item_prefixed_message_ids, repair_call_prefixed_function_item_ids,
    repair_custom_tool_item_ids, AdapterError, AdapterRequestContext, AdapterResponse,
    PreparedAdapterRequest,
};
use crate::runtime::{AccountTransport, AuthenticatedKey, AuthorizedRequestError};
use crate::scheduler::rotation::ExecutionCertainty;
use crate::scheduler::rotation::SharedRequestBudget;
use crate::usage::ReasoningEffortDiagnostics;
use crate::{Error, GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

pub(super) struct RequestExecution {
    pub(super) tool_policy: RequestToolPolicy,
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
    pub(super) wire_api: WireApi,
    pub(super) responses_lite: Option<HeaderValue>,
    pub(super) allow_previous_response_reset: bool,
    pub(super) attempt_offset: u16,
    pub(super) budget: SharedRequestBudget,
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

/// The Excel transport wraps tools inside Responses. Unwrap it before the
/// regular protocol adapter sees the response. For streamed client protocols,
/// feed the completed Responses response through that adapter's SSE bridge.
struct CompletedBasisPointsResponse {
    bytes: Vec<u8>,
    bridge_response: Option<AdapterResponse>,
    stream: Option<Vec<u8>>,
}

fn translate_basis_points_completed(
    adapter_request: PreparedAdapterRequest,
    upstream_bytes: &[u8],
    responses_request: &Value,
    stream: bool,
) -> Result<CompletedBasisPointsResponse, AdapterError> {
    let responses_bytes =
        super::basis_points::translate_response(upstream_bytes, responses_request)?;
    if !stream {
        let (bytes, response) = translate_completed_response(adapter_request, responses_bytes)?;
        return Ok(CompletedBasisPointsResponse {
            bytes,
            bridge_response: response,
            stream: None,
        });
    }
    let responses_stream = super::basis_points::synthetic_stream(&responses_bytes)?;
    let Some(mut bridge) = adapter_request.into_stream_bridge() else {
        return Ok(CompletedBasisPointsResponse {
            bytes: responses_bytes,
            bridge_response: None,
            stream: Some(responses_stream),
        });
    };
    bridge.push(&responses_stream);
    bridge.finish();
    let completed = bridge
        .completed()
        .cloned()
        .ok_or_else(AdapterError::upstream_stream_invalid)?;
    let mut client_stream = Vec::new();
    while let Some(event) = bridge.pop_output() {
        client_stream.extend_from_slice(&event);
    }
    let bytes = serde_json::to_vec(&completed.response_body)
        .map_err(|_| AdapterError::upstream_response_invalid())?;
    Ok(CompletedBasisPointsResponse {
        bytes,
        bridge_response: Some(AdapterResponse::Translated(completed)),
        stream: Some(client_stream),
    })
}

pub(super) async fn execute_request(context: RequestExecution) -> Response<Body> {
    let RequestExecution {
        mut tool_policy,
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
        wire_api,
        responses_lite,
        allow_previous_response_reset,
        attempt_offset,
        budget,
    } = context;
    let mut tried: HashSet<String> = Default::default();
    let mut attempt = attempt_offset;
    let mut confirmed_response_missing = false;
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
    let mut retry_window_expired = false;
    // Automatic Lite is safe only when every configured route in this key
    // scope is an official account with confirmed Lite support. Explicit
    // client Lite headers remain authoritative, but a mixed or partly unknown
    // pool must use full Responses so fallback preserves its tool/context
    // contract.
    let automatic_responses_lite = wire_api == WireApi::Responses
        && runtime.codex_model_responses_routes_all_support_lite(&key, &resolved_model);
    let mut has_unpaired_tool_output = !unpaired_tool_output_ids(&request).is_empty();
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
        operation: crate::scheduler::rotation::RotationOperation::Text,
        exclusions: &HashSet::new(),
    };

    loop {
        budget.retain_input_bytes(crate::gateway::request_body::retained_request_bytes(
            &request,
        ));
        budget.configure_retry_window(
            runtime.route_recovery_window_ms(),
            runtime.route_recovery_enabled(),
        );
        if !budget.can_dispatch() {
            break;
        }
        // Recovery can deliberately remove an unusable opaque response id.
        // Derive continuation semantics from the request that will actually be
        // sent on this attempt, rather than from its original payload.
        let has_previous_response_id = request_has_previous_response_id(wire_api, &request);
        // The gateway setting is live for every text protocol. Turning it off
        // wakes an already-waiting request on its next availability event.
        let retry_until_available = runtime.route_recovery_enabled();
        // Every pre-output retry (including SSE and transport failures) must
        // release a replayable request's optional tool affinity. Otherwise
        // selection stops at its already-tried owner despite healthy routes.
        // Opaque response references and unpaired tool outputs stay pinned.
        if !requires_affinity_owner
            && last_failure
                .as_ref()
                .is_some_and(|failure| failure_category_requires_cooldown(failure.category))
        {
            response_affinity_key = None;
        }
        let (incompatible, admission_error) = super::compatibility::incompatible_routes(
            &runtime,
            &key,
            &resolved_model,
            wire_api,
            &request,
            stream,
            &service_tier_policy,
            now_ms(),
        );
        let mut selection_exclusions = tried.clone();
        selection_exclusions.extend(incompatible.iter().cloned());
        if admission_error.is_some() {
            last_adapter_error = admission_error;
        }
        let selected = runtime
            .select_and_reserve_with_budget(
                &key,
                &resolved_model,
                candidate_protocols(wire_api),
                &selection_exclusions,
                (
                    response_affinity_key.as_deref(),
                    prompt_affinity_key.as_deref(),
                ),
                now_ms(),
                &budget,
            )
            .await;
        let Some((selected, lease)) = selected else {
            if let Some(error) = crate::gateway::errors::admission_error(&budget) {
                return error;
            }
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
                && drop_materialized_previous_response_id(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    now_ms(),
                )
            {
                model_switch_reset_attempted = true;
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
                        response_affinity_key = None;
                        requires_affinity_owner = false;
                        has_unpaired_tool_output = false;
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
                && drop_materialized_previous_response_id(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    now_ms(),
                )
            {
                model_switch_reset_attempted = true;
                response_affinity_key = None;
                requires_affinity_owner = false;
                continue;
            }
            if requires_affinity_owner
                && response_affinity_key.as_deref().and_then(|affinity_key| {
                    runtime.response_affinity_owner_supports_route(
                        &key,
                        affinity_key,
                        &resolved_model,
                        candidate_protocols(wire_api),
                        now_ms(),
                    )
                }) == Some(false)
            {
                return api_error(
                    StatusCode::CONFLICT,
                    RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
                    RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
                );
            }
            if wait_for_recovery(
                &budget,
                &CandidateRetryContext {
                    exclusions: &incompatible,
                    ..retry_context
                },
                &mut tried,
                response_affinity_key.as_deref(),
            )
            .await
            {
                continue;
            }
            if should_wait_for_candidate_availability(
                retry_until_available,
                &last_failure,
                last_adapter_error.is_some(),
                has_previous_response_id,
            ) {
                if !wait_for_candidate_retry(
                    &budget,
                    &retry_context,
                    &mut tried,
                    response_affinity_key.as_deref(),
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
                    crate::scheduler::rotation::RotationOperation::Text,
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
            service_tier_policy.select_for_model(&runtime, &route.source_model);
        service_tier_policy.prepare_for_candidate(&mut request, selected_service_tier, wire_api);
        route.half_open_probe = selected.half_open_probe;
        route.routing = Some(selected.diagnostics);
        route.client_context_id = client_context_id.clone();
        route.service_tier =
            service_tier_policy.effective_tier(&request, selected_service_tier, wire_api);
        let selected_error_origin = route_error_origin(&route);
        let source_model = route.source_model.clone();
        debug_assert_eq!(wire_api, route.wire_api);
        let account_route = route.account_id.is_some();
        let basis_points_route = route.account_transport == AccountTransport::ExcelBasisPoints;
        if basis_points_route {
            if responses_lite.is_some() {
                last_adapter_error =
                    Some(AdapterError::parameter_unsupported_for("responses_lite"));
                continue;
            }
            if let Some(error) = super::compatibility::basis_points_route_error(
                &request,
                stream,
                &service_tier_policy,
                selected_service_tier,
            ) {
                last_adapter_error = Some(error);
                continue;
            }
        }
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
                    error_codes::INVALID_REQUEST,
                );
            };
            if !responses_lite_parallel_tool_calls_valid(object) {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    "responses Lite requires parallel_tool_calls to be a boolean",
                    error_codes::INVALID_REQUEST,
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
        if account_route {
            let upstream_body = adapter_request.upstream_body_mut();
            let Value::Object(object) = upstream_body else {
                unreachable!("request object was validated before execution")
            };
            normalize_account_request(object, route_responses_lite.is_some());
        }
        let allow_deferred_tool_search = wire_api == WireApi::Responses
            && route.wire_api == WireApi::Responses
            && route.adapter.is_passthrough()
            && !basis_points_route;
        if let Err(message) =
            tool_policy.apply_adapter(&mut adapter_request, allow_deferred_tool_search)
        {
            return api_error(
                StatusCode::BAD_REQUEST,
                message,
                error_codes::INVALID_REQUEST,
            );
        }
        let basis_points_request =
            basis_points_route.then(|| adapter_request.upstream_body().clone());
        if basis_points_route {
            let prepared =
                match super::basis_points::prepare_request(adapter_request.upstream_body()) {
                    Ok(prepared) => prepared,
                    Err(error) if error.is_route_incompatible() => {
                        last_adapter_error = Some(error);
                        continue;
                    }
                    Err(error) => return adapter_error_response(error),
                };
            *adapter_request.upstream_body_mut() = prepared;
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
                error_codes::INVALID_REQUEST,
            );
        };
        let tool_use = tool_policy.diagnostics.clone();

        let upstream_stream = stream || (account_route && !basis_points_route);
        let started = Instant::now();
        let client = runtime.request_client(&route.candidate_id);
        let mut upstream_headers = if basis_points_route {
            HeaderMap::new()
        } else if adapter_request.requires_bridge_headers() {
            match route.adapter.upstream_protocol(wire_api) {
                crate::UpstreamProtocol::Messages => {
                    forwarded_bridge_messages_headers(&forwarded_headers)
                }
                crate::UpstreamProtocol::GeminiGenerateContent => {
                    forwarded_bridge_gemini_headers(&forwarded_headers)
                }
                _ => HeaderMap::new(),
            }
        } else {
            forwarded_headers.clone()
        };
        for (name, value) in &route.upstream_headers {
            upstream_headers.insert(name.clone(), value.clone());
        }
        let turn_scope = (account_route
            && !basis_points_route
            && wire_api == WireApi::Responses
            && route.adapter.is_passthrough())
        .then(|| {
            request_scope(
                &key.id,
                &forwarded_headers,
                route.account_id.as_deref(),
                &route.source_model,
            )
        })
        .flatten();
        if turn_scope.is_none() {
            upstream_headers.remove(CODEX_TURN_STATE_HEADER);
        }
        if account_route && !basis_points_route {
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
        if account_route && !basis_points_route {
            if let Some(value) = route_responses_lite.as_ref() {
                upstream_request = upstream_request.header(CODEX_RESPONSES_LITE_HEADER, value);
            }
        }
        let upstream = runtime
            .send_authorized_request(
                &route.candidate_id,
                upstream_request.body(request_body),
                (!basis_points_route)
                    .then(|| codex_client_version(&forwarded_headers))
                    .flatten(),
                turn_scope.as_ref(),
                Some(&budget),
                Some(&lease),
            )
            .await;
        // Includes internal auth replay; repair and recovery cannot refund a
        // real upstream dispatch just by changing their visible attempt count.
        attempt = u16::from(budget.dispatches());
        let upstream = match upstream {
            Ok(upstream) => {
                route.account_token_generation = upstream.account_token_generation;
                upstream.response
            }
            Err(error) => {
                let uncertain = error.execution_certainty() == ExecutionCertainty::Unknown;
                let exhausted = matches!(error, AuthorizedRequestError::DispatchBudgetExhausted);
                let failure = AttemptFailure::authorized_request(error);
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
                // Unknown remote acceptance is neither a health vote nor a
                // replayable error. A local exhausted budget is not a source
                // failure either.
                if uncertain || exhausted {
                    if uncertain {
                        lease.settle_rotation_unknown(now_ms());
                    }
                    emit_usage(&runtime, event);
                    return attempt_error_response(
                        failure,
                        None,
                        selected_error_origin,
                        &request_id,
                    );
                }
                let state = settle_attempt_failure(
                    &runtime,
                    &lease,
                    &source_model,
                    &failure,
                    &HeaderMap::new(),
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
                    let state = settle_status_failure(
                        &runtime,
                        &lease,
                        &source_model,
                        status,
                        failure.category,
                        &response_headers,
                        None,
                    );
                    apply_failure_state(&mut event, state);
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    last_failure_origin = selected_error_origin;
                    continue;
                }
                Err(error) => {
                    lease.settle_rotation_unknown(now_ms());
                    return upstream_body_error_response(&runtime, event, started, error);
                }
            };
            if try_repair_legacy_responses_call_ids(
                &mut request,
                wire_api,
                adapter_is_passthrough,
                status.is_client_error() && responses_tool_call_links_rejected(&bytes),
                &mut legacy_call_id_repair_attempted,
                &mut tried,
                &route.candidate_id,
                &mut has_unpaired_tool_output,
                &mut requires_affinity_owner,
            ) {
                lease.settle_rotation_repair(now_ms());
                continue;
            }
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && !function_item_id_repair_attempted
                && responses_function_item_id_requires_fc_prefix(&bytes)
                && repair_call_prefixed_function_item_ids(&mut request)
            {
                function_item_id_repair_attempted = true;
                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                lease.settle_rotation_repair(now_ms());
                continue;
            }
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && !custom_tool_item_id_repair_attempted
                && responses_custom_tool_item_id_requires_ctc_prefix(&bytes)
                && repair_custom_tool_item_ids(&mut request)
            {
                custom_tool_item_id_repair_attempted = true;
                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                lease.settle_rotation_repair(now_ms());
                continue;
            }
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && !message_item_id_repair_attempted
                && responses_message_item_id_requires_msg_prefix(&bytes)
                && remove_item_prefixed_message_ids(&mut request)
            {
                message_item_id_repair_attempted = true;
                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                lease.settle_rotation_repair(now_ms());
                continue;
            }
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            last_preserved_upstream_error = preserved_upstream_error(&failure, &bytes);
            let upstream_error =
                crate::usage::UpstreamErrorDetails::from_body(Some(status.as_u16()), &bytes);
            event.upstream_error = Some(upstream_error.clone());
            event.error_category = Some(failure.category.to_string());
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && is_deferred_tool_search_compatibility_error(status, &upstream_error)
                && tool_policy.prepare_deferred_fallback()
            {
                // An older or non-OpenAI Responses endpoint may reject the
                // standard tool-search fields. Retry once without changing
                // the selected policy, and only before output.
                event.tool_use.policy_fallback = true;
                emit_usage(&runtime, event);
                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
                lease.settle_rotation_repair(now_ms());
                continue;
            }
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
                        response_affinity_key = None;
                        requires_affinity_owner = false;
                        has_unpaired_tool_output = false;
                        tried.remove(&route.candidate_id);
                        lease.allow_rotation_repair();
                        emit_usage(&runtime, event);
                        last_failure = Some(failure);
                        last_failure_origin = selected_error_origin;
                        lease.settle_rotation_repair(now_ms());
                        continue;
                    }
                    Ok(false) => {}
                    Err(error) => return adapter_error_response(error),
                }
            }
            if wire_api == WireApi::Responses
                && has_previous_response_id
                && responses_tool_call_is_missing_output(&bytes)
                && recover_stale_tool_history(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    &bytes,
                    &mut stale_tool_history_recovered,
                )
            {
                response_affinity_key = None;
                requires_affinity_owner = false;
                has_unpaired_tool_output = false;
                tried.remove(&route.candidate_id);
                lease.allow_rotation_repair();
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
                lease.settle_rotation_repair(now_ms());
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
                && drop_materialized_previous_response_id(
                    &runtime,
                    &key.id,
                    &mut request,
                    &resolved_model,
                    now_ms(),
                )
            {
                model_switch_reset_attempted = true;
                response_affinity_key = None;
                requires_affinity_owner = false;
                emit_usage(&runtime, event);
                last_failure = Some(failure);
                last_failure_origin = selected_error_origin;
                lease.settle_rotation_repair(now_ms());
                continue;
            }
            let rejection_state = settle_attempt_failure(
                &runtime,
                &lease,
                &source_model,
                &failure,
                &response_headers,
            );
            let response_missing = previous_response_not_found(&bytes);
            let affinity_miss = recoverable_response_affinity_miss(
                status,
                has_previous_response_id,
                response_affinity_hit,
                response_missing,
            );
            // A Responses continuation normally has to stay on the creator
            // of `previous_response_id`.  When that owner is temporarily
            // unavailable, use the volatile native replay captured from the
            // successful turn before selecting another candidate.  This is
            // the safe hand-off path: the new candidate receives the
            // materialized conversation, never a foreign opaque response id.
            if wire_api == WireApi::Responses
                && adapter_is_passthrough
                && has_previous_response_id
                && response_affinity_hit
                && requires_affinity_owner
                && !native_replay_attempted
                && (retryable_failure(status, failure.category, has_previous_response_id)
                    || (affinity_miss && response_missing))
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
                        response_affinity_key = None;
                        requires_affinity_owner = false;
                        has_unpaired_tool_output = false;
                        if response_missing {
                            // The owner is healthy but has lost its opaque
                            // response id. It can safely accept the
                            // materialized conversation on the next attempt.
                            tried.remove(&route.candidate_id);
                            lease.allow_rotation_repair();
                            event.error_category =
                                Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
                        } else {
                            let state = rejection_state.clone();
                            apply_failure_state(&mut event, state);
                        }
                        // A retryable transport/availability failure leaves
                        // the owner in `tried`: replay has materialized the
                        // conversation specifically so the next attempt can
                        // be handed to a different slot.
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
                    runtime.invalidate_response_affinity(response_affinity_key.as_deref());
                    event.error_category = Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
                } else {
                    let state = rejection_state.clone();
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
                return attempt_error_response(
                    failure,
                    last_preserved_upstream_error.as_ref(),
                    selected_error_origin,
                    &request_id,
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
                relay_account_response_header(&forwarded_headers, &response_headers, &mut response);
            }
            return response;
        }

        // Managed ChatGPT accounts request the upstream Responses stream even
        // for a buffered client request.  Buffer based on the client contract,
        // then normalize either a JSON response or the completed SSE stream
        // into the same response path.
        if !stream || basis_points_route {
            let bytes = match crate::transport::collect_limited(
                upstream,
                crate::runtime::MAX_NON_STREAM_BODY_BYTES,
            )
            .await
            {
                Ok(bytes) => bytes,
                Err(error) => {
                    lease.settle_rotation_unknown(now_ms());
                    let too_large = matches!(error, Error::UpstreamBodyTooLarge);
                    let failure = AttemptFailure::body();
                    let state = current_failure_state(&runtime, &route.candidate_id, &source_model);
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
                            error_codes::UPSTREAM_BODY_TOO_LARGE.to_string()
                        } else {
                            error_codes::UPSTREAM_BODY.to_string()
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
            let bytes = {
                match completed_upstream_response(&bytes, account_route) {
                    Ok(bytes) => bytes,
                    Err(upstream_failure) => {
                        let mut failure = upstream_failure.failure;
                        failure.execution = upstream_failure.execution;
                        last_preserved_upstream_error = upstream_failure.preserved;
                        let state = Some(settle_attempt_failure(
                            &runtime,
                            &lease,
                            &source_model,
                            &failure,
                            &response_headers,
                        ));
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
                        event.upstream_error =
                            upstream_failure.upstream_error.map(|mut details| {
                                details.http_status = Some(status.as_u16());
                                details
                            });
                        if wire_api == WireApi::Responses
                            && response_affinity_hit
                            && has_previous_response_id
                            && !native_replay_attempted
                            && failure.category == error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
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
                                    response_affinity_key = None;
                                    requires_affinity_owner = false;
                                    has_unpaired_tool_output = false;
                                    tried.remove(&route.candidate_id);
                                    lease.allow_rotation_repair();
                                    event.error_category =
                                        Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
                                    emit_usage(&runtime, event);
                                    last_failure = Some(failure);
                                    last_failure_origin = selected_error_origin;
                                    continue;
                                }
                                Ok(false) => {}
                                Err(error) => return adapter_error_response(error),
                            }
                        }
                        if let Some(state) = state {
                            apply_failure_state(&mut event, state);
                        }
                        emit_usage(&runtime, event);
                        if failure_category_is_request_terminal(failure.category) {
                            return attempt_error_response(
                                failure,
                                last_preserved_upstream_error.as_ref(),
                                selected_error_origin,
                                &request_id,
                            );
                        }
                        last_failure = Some(failure);
                        last_failure_origin = selected_error_origin;
                        continue;
                    }
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
            // Accounting reads the actual upstream counters before translation.
            populate_tokens(&mut event, &bytes);
            let translated = if let Some(responses_request) = basis_points_request.as_ref() {
                translate_basis_points_completed(adapter_request, &bytes, responses_request, stream)
            } else {
                translate_completed_response(adapter_request, bytes).map(
                    |(bytes, bridge_response)| CompletedBasisPointsResponse {
                        bytes,
                        bridge_response,
                        stream: None,
                    },
                )
            };
            let CompletedBasisPointsResponse {
                bytes,
                bridge_response,
                stream: basis_points_stream,
            } = match translated {
                Ok(response) => response,
                Err(error) => {
                    event.success = false;
                    event.http_status = StatusCode::BAD_GATEWAY.as_u16();
                    event.error_category = Some(error.code().to_string());
                    emit_usage(&runtime, event);
                    lease.settle_rotation_terminal(now_ms());
                    return adapter_error_response_for_origin(error, selected_error_origin);
                }
            };
            let recovered = runtime.record_success_with_metrics(
                &route.candidate_id,
                &source_model,
                now_ms(),
                event.output_tokens,
                event.generation_ms.unwrap_or(event.latency_ms),
            );
            event.consecutive_failures = recovered.then_some(0);
            lease.settle_rotation_success(now_ms());
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
            if let Some(stream_body) = basis_points_stream {
                let mut response =
                    proxy_sse_response(status, &response_headers, Body::from(stream_body));
                relay_account_response_header(&forwarded_headers, &response_headers, &mut response);
                return response;
            }
            if account_route || !adapter_is_passthrough {
                let mut response =
                    proxy_json_response(status, &response_headers, Body::from(bytes));
                if account_route && adapter_is_passthrough {
                    relay_account_response_header(
                        &forwarded_headers,
                        &response_headers,
                        &mut response,
                    );
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
                if account_id.is_some() {
                    relay_account_response_header(&forwarded_headers, &headers, &mut response);
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
                let tool_links_rejected = bootstrap_failure.responses_tool_call_links_rejected;
                let mut failure = bootstrap_failure.failure;
                failure.execution = bootstrap_failure.execution;
                last_preserved_upstream_error = bootstrap_failure.preserved;
                let upstream_error = bootstrap_failure.upstream_error;
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
                event.upstream_error = upstream_error.map(|mut details| {
                    details.http_status = Some(status.as_u16());
                    details
                });
                if try_repair_legacy_responses_call_ids(
                    &mut request,
                    wire_api,
                    adapter_is_passthrough,
                    tool_links_rejected,
                    &mut legacy_call_id_repair_attempted,
                    &mut tried,
                    &route.candidate_id,
                    &mut has_unpaired_tool_output,
                    &mut requires_affinity_owner,
                ) {
                    lease.settle_rotation_repair(now_ms());
                    continue;
                }
                if wire_api == WireApi::Responses
                    && adapter_is_passthrough
                    && has_previous_response_id
                    && !native_replay_attempted
                    && ((contains_tool_call_output(&request) && zenith_gateway_invalid_request)
                        || (response_affinity_hit
                            && failure.category
                                == error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND))
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
                            if failure.category == error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
                            {
                                event.error_category =
                                    Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
                            }
                            response_affinity_key = None;
                            requires_affinity_owner = false;
                            has_unpaired_tool_output = false;
                            tried.remove(&route.candidate_id);
                            lease.allow_rotation_repair();
                            emit_usage(&runtime, event);
                            last_failure = Some(failure);
                            last_failure_origin = selected_error_origin;
                            lease.settle_rotation_repair(now_ms());
                            continue;
                        }
                        Ok(false) => {}
                        Err(error) => return adapter_error_response(error),
                    }
                }
                if wire_api == WireApi::Responses
                    && has_previous_response_id
                    && missing_tool_output
                    && last_preserved_upstream_error.as_ref().is_some_and(|error| {
                        recover_stale_tool_history(
                            &runtime,
                            &key.id,
                            &mut request,
                            &resolved_model,
                            error.message.as_bytes(),
                            &mut stale_tool_history_recovered,
                        )
                    })
                {
                    response_affinity_key = None;
                    requires_affinity_owner = false;
                    has_unpaired_tool_output = false;
                    tried.remove(&route.candidate_id);
                    lease.allow_rotation_repair();
                    emit_usage(&runtime, event);
                    last_failure = Some(failure);
                    last_failure_origin = selected_error_origin;
                    lease.settle_rotation_repair(now_ms());
                    continue;
                }
                let failure_state = settle_attempt_failure(
                    &runtime,
                    &lease,
                    &source_model,
                    &failure,
                    &response_headers,
                );
                apply_failure_state(&mut event, failure_state);
                emit_usage(&runtime, event);
                if failure_category_is_request_terminal(failure.category) {
                    return attempt_error_response(
                        failure,
                        last_preserved_upstream_error.as_ref(),
                        selected_error_origin,
                        &request_id,
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
    {
        let mut reset_request = request;
        if drop_materialized_previous_response_id(
            &runtime,
            &key.id,
            &mut reset_request,
            &resolved_model,
            now_ms(),
        ) {
            return Box::pin(execute_request(RequestExecution {
                tool_policy,
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
                wire_api,
                responses_lite,
                allow_previous_response_reset: false,
                attempt_offset: attempt,
                budget,
            }))
            .await;
        }
        return api_error(
            StatusCode::CONFLICT,
            RESPONSE_CONTINUATION_UNAVAILABLE_MESSAGE,
            RESPONSE_CONTINUATION_UNAVAILABLE_CODE,
        );
    }

    if last_failure.is_none() {
        if let Some(error) = last_adapter_error {
            return adapter_error_response(error);
        }
    }
    if let Some(error) = crate::gateway::errors::admission_error(&budget) {
        return error;
    }
    let failure = if retry_window_expired {
        AttemptFailure::classified_with_hint(
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
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
        crate::scheduler::rotation::RotationOperation::Text,
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
            crate::gateway::errors::retryable_recovery_wait(
                failure.status,
                failure.category,
                has_previous_response_id,
            )
        })
}

fn recover_stale_tool_history(
    runtime: &GatewayRuntime,
    local_key_id: &str,
    request: &mut Value,
    model: &str,
    upstream_error: &[u8],
    recovered: &mut bool,
) -> bool {
    let stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if *recovered
        || !replay_and_prune_stale_tool_history(
            runtime,
            local_key_id,
            request,
            model,
            now_ms(),
            stream,
            upstream_error,
        )
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
    upstream_rejected_tool_links: bool,
    repair_attempted: &mut bool,
    tried: &mut HashSet<String>,
    candidate_id: &str,
    has_unpaired_tool_output: &mut bool,
    requires_affinity_owner: &mut bool,
) -> bool {
    if wire_api != WireApi::Responses
        || !adapter_is_passthrough
        || *repair_attempted
        || !upstream_rejected_tool_links
        || !repair_legacy_responses_call_ids(request)
    {
        return false;
    }

    *repair_attempted = true;
    tried.remove(candidate_id);
    *has_unpaired_tool_output = !unpaired_tool_output_ids(request).is_empty();
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
    *request = match replay.replay_request(request, model, stream) {
        Ok(request) => request,
        Err(error) if error.code() == error_codes::ADAPTER_CONTINUATION_MISMATCH => {
            return Ok(false)
        }
        Err(error) => return Err(error),
    };
    *attempted = true;
    Ok(true)
}

pub(super) fn adapter_error_response(error: AdapterError) -> Response<Body> {
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
    let message = error
        .parameter()
        .map(|parameter| format!("{} (parameter: {parameter})", error.message()));
    super::super::errors::api_error_with_parameter(
        status,
        message.as_deref().unwrap_or(error.message()),
        error.code(),
        error.code(),
        origin,
        None,
        error.parameter(),
    )
}

#[cfg(test)]
mod tests {
    use super::{
        requested_reasoning_effort, should_wait_for_candidate_availability,
        translate_basis_points_completed,
    };
    use crate::gateway::errors::AttemptFailure;
    use crate::{
        AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter, WireApi,
    };
    use axum::http::StatusCode;
    use serde_json::json;

    #[test]
    fn basis_points_completed_json_and_stream_use_the_client_protocol() {
        let upstream = serde_json::to_vec(&json!({
            "id": "resp_1", "model": "test", "status": "completed",
            "output": [{"type":"message","id":"msg_1","role":"assistant","status":"completed",
                "content":[{"type":"output_text","text":"Hello","annotations":[]}]}],
            "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
        }))
        .unwrap();
        for client in WireApi::ALL {
            let input = match client {
                WireApi::Responses => json!({"model":"test","input":"Hi"}),
                WireApi::ChatCompletions => {
                    json!({"model":"test","messages":[{"role":"user","content":"Hi"}]})
                }
                WireApi::Messages => {
                    json!({"model":"test","max_tokens":64,"messages":[{"role":"user","content":"Hi"}]})
                }
                WireApi::Gemini => json!({"contents":[{"role":"user","parts":[{"text":"Hi"}]}]}),
            };
            for stream in [false, true] {
                let adapter = SourceAdapter::between(client, WireApi::Responses).unwrap();
                let prepared = adapter
                    .prepare_request(AdapterRequestContext {
                        client_wire_api: client,
                        request: &input,
                        model: "test",
                        stream,
                        reasoning_mode: MessagesReasoningMode::Adaptive,
                        cache_write_ttl: CacheWriteTtl::Provider,
                        previous: None,
                        response_scope: "test-account",
                        response_id_seed: "test-request",
                    })
                    .unwrap();
                let responses_request = prepared.upstream_body().clone();
                let result = translate_basis_points_completed(
                    prepared,
                    &upstream,
                    &responses_request,
                    stream,
                )
                .unwrap();
                let body: serde_json::Value = serde_json::from_slice(&result.bytes).unwrap();
                let events = result.stream.map(|bytes| String::from_utf8(bytes).unwrap());
                match client {
                    WireApi::Responses => {
                        assert_eq!(body["output"][0]["content"][0]["text"], "Hello");
                        if let Some(events) = &events {
                            assert!(events.contains("response.completed"));
                        }
                    }
                    WireApi::ChatCompletions => {
                        assert_eq!(body["choices"][0]["message"]["content"], "Hello");
                        if let Some(events) = &events {
                            assert!(events.contains("chat.completion.chunk"));
                        }
                    }
                    WireApi::Messages => {
                        assert_eq!(body["content"][0]["text"], "Hello");
                        if let Some(events) = &events {
                            assert!(events.contains("message_stop"));
                        }
                    }
                    WireApi::Gemini => {
                        assert_eq!(
                            body["candidates"][0]["content"]["parts"][0]["text"],
                            "Hello"
                        );
                        if let Some(events) = &events {
                            assert!(events.contains("candidates"));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn basis_points_messages_tool_call_is_unwrapped_before_protocol_translation() {
        let input = json!({
            "model": "test", "max_tokens": 64,
            "messages": [{"role":"user","content":"Find it"}],
            "tools": [{"name":"lookup","description":"Lookup","input_schema":{
                "type":"object","properties":{"q":{"type":"string"}}
            }}]
        });
        let code = json!({"tool":"lookup","args":{"q":"needle"}}).to_string();
        let upstream = serde_json::to_vec(&json!({
            "id": "resp_1", "model": "test", "status": "completed",
            "output": [{"type":"function_call","id":"fc_outer","call_id":"call_1",
                "name":"run_officejs","arguments":json!({"code":code}).to_string()}],
            "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
        }))
        .unwrap();
        for stream in [false, true] {
            let prepared = SourceAdapter::MessagesToResponses
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Messages,
                    request: &input,
                    model: "test",
                    stream,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "test-account",
                    response_id_seed: "test-request",
                })
                .unwrap();
            let responses_request = prepared.upstream_body().clone();
            let result =
                translate_basis_points_completed(prepared, &upstream, &responses_request, stream)
                    .unwrap();
            let body: serde_json::Value = serde_json::from_slice(&result.bytes).unwrap();
            assert_eq!(body["content"][0]["type"], "tool_use");
            assert_eq!(body["content"][0]["name"], "lookup");
            assert_eq!(body["content"][0]["input"]["q"], "needle");
            if let Some(events) = result.stream {
                let events = String::from_utf8(events).unwrap();
                assert!(events.contains("tool_use"));
                assert!(events.contains("message_stop"));
            }
        }
    }

    #[test]
    fn basis_points_messages_tool_result_keeps_the_call_link() {
        let input = json!({
            "model":"test", "max_tokens":64,
            "tools":[{"name":"lookup","input_schema":{"type":"object"}}],
            "messages":[
                {"role":"user","content":"Find it"},
                {"role":"assistant","content":[{"type":"tool_use","id":"call_1","name":"lookup","input":{"q":"needle"}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_1","content":"Found"}]}
            ]
        });
        let prepared = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &input,
                model: "test",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "test-account",
                response_id_seed: "test-request",
            })
            .unwrap();
        let basis_points =
            super::super::basis_points::prepare_request(prepared.upstream_body()).unwrap();
        let items = basis_points["input"].as_array().unwrap();
        assert!(items.iter().any(|item| {
            item["type"] == "function_call"
                && item["name"] == "run_officejs"
                && item["call_id"] == "call_1"
        }));
        assert!(items.iter().any(|item| {
            item["type"] == "function_call_output"
                && item["call_id"] == "call_1"
                && item["output"] == "Found"
        }));
    }

    #[tokio::test]
    async fn adapter_error_response_exposes_safe_parameter_name() {
        let response = super::adapter_error_response(
            crate::AdapterError::parameter_unsupported_for("text.verbosity"),
        );
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["error"]["param"], "text.verbosity");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("text.verbosity"));
        assert_eq!(body["error"]["code"], "adapter_parameter_unsupported");
    }

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
}
