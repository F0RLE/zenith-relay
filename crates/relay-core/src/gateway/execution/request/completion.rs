use super::prelude::*;
use super::recovery::{
    adapter_error_response, adapter_error_response_for_origin, replay_native_tool_continuation,
};
use super::retry::{
    basis_points_relay_error_response, handle_basis_points_relay_retry, mark_adapter_failure,
    BasisPointsRelayRetryContext,
};
use super::translate::{
    translate_basis_points_completed, translate_completed_response, CompletedBasisPointsResponse,
};

pub(super) enum CompletionStep {
    Continue,
    Respond(Response<Body>),
}

pub(super) struct BufferedCompletionInput<'a> {
    pub(super) upstream: reqwest::Response,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) lease: &'a CandidateLease,
    pub(super) route: &'a crate::runtime::ExecutorRoute,
    pub(super) source_model: &'a str,
    pub(super) request_id: &'a str,
    pub(super) attempt: u16,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) reasoning_effort: &'a ReasoningEffortDiagnostics,
    pub(super) requested_model: &'a str,
    pub(super) tool_use: &'a crate::usage::ToolUseDiagnostics,
    pub(super) started: Instant,
    pub(super) status: StatusCode,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) account_route: bool,
    pub(super) wire_api: WireApi,
    pub(super) request: &'a mut Value,
    pub(super) adapter_is_passthrough: bool,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) response_affinity_key: &'a mut Option<String>,
    pub(super) requires_affinity_owner: &'a mut bool,
    pub(super) has_unpaired_tool_output: &'a mut bool,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_failure_origin: &'a mut ErrorOrigin,
    pub(super) last_preserved_upstream_error: &'a mut Option<PreservedUpstreamError>,
    pub(super) has_previous_response_id: bool,
    pub(super) basis_points_route: bool,
    pub(super) basis_points_request: &'a Option<Value>,
    pub(super) stream: bool,
    pub(super) adapter_request: PreparedAdapterRequest,
    pub(super) basis_points_relay_retry_attempted: &'a mut bool,
    pub(super) basis_points_relay_retry_parameter: &'a mut Option<&'static str>,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) last_adapter_error: &'a mut Option<AdapterError>,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) response_affinity_hit: bool,
    pub(super) prompt_affinity_key: &'a Option<String>,
    pub(super) summarize: bool,
    pub(super) client_stream: bool,
    pub(super) forwarded_headers: &'a HeaderMap,
}

/// Buffer one successful upstream body and translate it for the client.
/// A recoverable rejection continues the attempt loop instead of responding.
pub(super) async fn complete_buffered_response(
    input: BufferedCompletionInput<'_>,
) -> CompletionStep {
    let BufferedCompletionInput {
        upstream,
        runtime,
        lease,
        route,
        source_model,
        request_id,
        attempt,
        key,
        reasoning_effort,
        requested_model,
        tool_use,
        started,
        status,
        response_headers,
        account_route,
        wire_api,
        request,
        adapter_is_passthrough,
        repairs,
        response_affinity_key,
        requires_affinity_owner,
        has_unpaired_tool_output,
        last_failure,
        last_failure_origin,
        last_preserved_upstream_error,
        has_previous_response_id,
        basis_points_route,
        basis_points_request,
        stream,
        adapter_request,
        basis_points_relay_retry_attempted,
        basis_points_relay_retry_parameter,
        tried,
        last_adapter_error,
        selected_error_origin,
        response_affinity_hit,
        prompt_affinity_key,
        summarize,
        client_stream,
        forwarded_headers,
    } = input;
    let native_replay_attempted = &mut repairs.native_replay;
    let usage = |success, http_status, error_category| {
        usage_event(
            UsageAttempt {
                request_id,
                attempt,
                local_key_id: &key.id,
                route,
                reasoning_effort: Some(reasoning_effort),
                requested_model,
                tool_use: tool_use.clone(),
            },
            success,
            http_status,
            error_category,
            started.elapsed().as_millis() as u64,
        )
    };
    let bytes = match read_completed_body(
        upstream,
        &mut CompletedBodyRead {
            runtime,
            lease,
            route,
            source_model,
            request_id,
            key,
            request,
            response_headers,
            status,
            wire_api,
            stream,
            account_route,
            response_affinity_hit,
            has_previous_response_id,
            selected_error_origin,
            native_replay_attempted,
            response_affinity_key,
            requires_affinity_owner,
            has_unpaired_tool_output,
            tried,
            last_failure,
            last_failure_origin,
            last_preserved_upstream_error,
        },
        &usage,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(step) => return step,
    };
    let mut event = usage(true, status.as_u16(), None);
    // Accounting reads the actual upstream counters before translation.
    populate_tokens(&mut event, &bytes);
    let basis_points_retry_body = basis_points_route.then(|| bytes.clone());
    let translated = if let Some(responses_request) = basis_points_request.as_ref() {
        translate_basis_points_completed(adapter_request, &bytes, responses_request, stream)
    } else {
        translate_completed_response(adapter_request, bytes).map(|(bytes, bridge_response)| {
            CompletedBasisPointsResponse {
                bytes,
                bridge_response,
                stream: None,
            }
        })
    };
    let CompletedBasisPointsResponse {
        mut bytes,
        bridge_response,
        stream: basis_points_stream,
    } = match translated {
        Ok(response) => response,
        Err(error) => {
            if basis_points_route {
                match handle_basis_points_relay_retry(
                    error,
                    basis_points_retry_body.as_deref().unwrap_or_default(),
                    event,
                    BasisPointsRelayRetryContext {
                        attempted: basis_points_relay_retry_attempted,
                        parameter: basis_points_relay_retry_parameter,
                        runtime,
                        tried,
                        candidate_id: &route.candidate_id,
                        lease,
                        last_adapter_error,
                    },
                ) {
                    Ok(()) => return CompletionStep::Continue,
                    Err(pair) => {
                        let (error, event) = *pair;
                        return CompletionStep::Respond(basis_points_relay_error_response(
                            error,
                            event,
                            runtime,
                            lease,
                            selected_error_origin,
                        ));
                    }
                }
            }
            emit_usage(runtime, mark_adapter_failure(event, &error));
            lease.settle_rotation_terminal(now_ms());
            return CompletionStep::Respond(adapter_error_response_for_origin(
                error,
                selected_error_origin,
            ));
        }
    };
    if summarize {
        if let Err(error) = crate::protocol::wrap_compaction_response_bytes(&mut bytes) {
            emit_usage(runtime, mark_adapter_failure(event, &error));
            lease.settle_rotation_terminal(now_ms());
            return CompletionStep::Respond(adapter_error_response_for_origin(
                error,
                selected_error_origin,
            ));
        }
    }
    let recovered = runtime.record_success_with_metrics(
        &route.candidate_id,
        source_model,
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
    emit_usage(runtime, event);
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
    if wire_api == WireApi::Responses {
        bind_responses_turn(
            runtime,
            &key.id,
            &route.candidate_id,
            request,
            source_model,
            &bytes,
            adapter_is_passthrough,
        );
    }
    buffered_client_response(
        status,
        response_headers,
        forwarded_headers,
        bytes,
        basis_points_stream,
        account_route,
        adapter_is_passthrough,
        summarize,
        client_stream,
        selected_error_origin,
    )
}

#[allow(clippy::too_many_arguments)]
fn buffered_client_response(
    status: StatusCode,
    response_headers: &HeaderMap,
    forwarded_headers: &HeaderMap,
    bytes: Vec<u8>,
    basis_points_stream: Option<Vec<u8>>,
    account_route: bool,
    adapter_is_passthrough: bool,
    summarize: bool,
    client_stream: bool,
    selected_error_origin: ErrorOrigin,
) -> CompletionStep {
    if let Some(stream_body) = basis_points_stream {
        let mut response = proxy_sse_response(status, response_headers, Body::from(stream_body));
        relay_account_response_header(forwarded_headers, response_headers, &mut response);
        return CompletionStep::Respond(response);
    }
    if account_route || !adapter_is_passthrough {
        if summarize && client_stream {
            let stream_body = match super::super::basis_points::synthetic_stream(&bytes) {
                Ok(stream_body) => stream_body,
                Err(error) => {
                    return CompletionStep::Respond(adapter_error_response_for_origin(
                        error,
                        selected_error_origin,
                    ));
                }
            };
            let mut response =
                proxy_sse_response(status, response_headers, Body::from(stream_body));
            if account_route && adapter_is_passthrough {
                relay_account_response_header(forwarded_headers, response_headers, &mut response);
            }
            return CompletionStep::Respond(response);
        }
        let mut response = proxy_json_response(status, response_headers, Body::from(bytes));
        if account_route && adapter_is_passthrough {
            relay_account_response_header(forwarded_headers, response_headers, &mut response);
        }
        return CompletionStep::Respond(response);
    }
    CompletionStep::Respond(proxy_response(status, response_headers, Body::from(bytes)))
}

struct CompletedBodyRead<'a> {
    runtime: &'a GatewayRuntime,
    lease: &'a CandidateLease,
    route: &'a crate::runtime::ExecutorRoute,
    source_model: &'a str,
    request_id: &'a str,
    key: &'a AuthenticatedKey,
    request: &'a mut Value,
    response_headers: &'a HeaderMap,
    status: StatusCode,
    wire_api: WireApi,
    stream: bool,
    account_route: bool,
    response_affinity_hit: bool,
    has_previous_response_id: bool,
    selected_error_origin: ErrorOrigin,
    native_replay_attempted: &'a mut bool,
    response_affinity_key: &'a mut Option<String>,
    requires_affinity_owner: &'a mut bool,
    has_unpaired_tool_output: &'a mut bool,
    tried: &'a mut HashSet<String>,
    last_failure: &'a mut Option<AttemptFailure>,
    last_failure_origin: &'a mut ErrorOrigin,
    last_preserved_upstream_error: &'a mut Option<PreservedUpstreamError>,
}

#[allow(clippy::result_large_err)]
async fn read_completed_body(
    upstream: reqwest::Response,
    read: &mut CompletedBodyRead<'_>,
    usage: &impl Fn(bool, u16, Option<String>) -> UsageEvent,
) -> Result<Vec<u8>, CompletionStep> {
    let bytes = match crate::transport::collect_limited(
        upstream,
        crate::runtime::MAX_NON_STREAM_BODY_BYTES,
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(error) => {
            read.lease.settle_rotation_unknown(now_ms());
            let too_large = matches!(error, Error::UpstreamBodyTooLarge);
            let failure = AttemptFailure::body();
            let state =
                current_failure_state(read.runtime, &read.route.candidate_id, read.source_model);
            let mut event = usage(
                false,
                StatusCode::BAD_GATEWAY.as_u16(),
                Some(if too_large {
                    error_codes::UPSTREAM_BODY_TOO_LARGE.to_string()
                } else {
                    error_codes::UPSTREAM_BODY.to_string()
                }),
            );
            apply_failure_state(&mut event, state);
            emit_usage(read.runtime, event);
            *read.last_failure = Some(failure);
            *read.last_failure_origin = read.selected_error_origin;
            return Err(CompletionStep::Continue);
        }
    };
    match completed_upstream_response(
        &bytes,
        read.account_route,
        read.account_route && read.runtime.block_degraded_routes_enabled(),
    ) {
        Ok(bytes) => Ok(bytes),
        Err(upstream_failure) => {
            let mut failure = upstream_failure.failure;
            failure.execution = upstream_failure.execution;
            *read.last_preserved_upstream_error = upstream_failure.preserved;
            let state = Some(settle_attempt_failure(
                read.runtime,
                read.lease,
                read.source_model,
                &failure,
                read.response_headers,
            ));
            let mut event = usage(
                false,
                failure.status.as_u16(),
                Some(failure.category.to_string()),
            );
            event.upstream_error = upstream_failure.upstream_error.map(|mut details| {
                details.http_status = Some(read.status.as_u16());
                details
            });
            if read.wire_api == WireApi::Responses
                && read.response_affinity_hit
                && read.has_previous_response_id
                && !*read.native_replay_attempted
                && failure.category == error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
            {
                match replay_native_tool_continuation(
                    read.runtime,
                    &read.key.id,
                    read.request,
                    read.route,
                    read.stream,
                    read.native_replay_attempted,
                ) {
                    Ok(true) => {
                        clear_materialized_continuation(
                            read.response_affinity_key,
                            read.requires_affinity_owner,
                            read.has_unpaired_tool_output,
                        );
                        read.tried.remove(&read.route.candidate_id);
                        read.lease.allow_rotation_repair();
                        event.error_category =
                            Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
                        emit_usage(read.runtime, event);
                        *read.last_failure = Some(failure);
                        *read.last_failure_origin = read.selected_error_origin;
                        return Err(CompletionStep::Continue);
                    }
                    Ok(false) => {}
                    Err(error) => {
                        return Err(CompletionStep::Respond(adapter_error_response(error)));
                    }
                }
            }
            if let Some(state) = state {
                apply_failure_state(&mut event, state);
            }
            emit_usage(read.runtime, event);
            if failure_category_is_request_terminal(failure.category) {
                return Err(CompletionStep::Respond(attempt_error_response(
                    failure,
                    read.last_preserved_upstream_error.as_ref(),
                    read.selected_error_origin,
                    read.request_id,
                )));
            }
            *read.last_failure = Some(failure);
            *read.last_failure_origin = read.selected_error_origin;
            Err(CompletionStep::Continue)
        }
    }
}
