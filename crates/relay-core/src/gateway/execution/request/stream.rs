use super::prelude::*;
use super::recovery::{
    LegacyCallIdRepair, adapter_error_response, mark_adapter_failure, recover_stale_tool_history,
    replay_native_tool_continuation, try_repair_legacy_responses_call_ids,
};
use crate::gateway::streaming::UpstreamStream;
use crate::runtime::ExecutorRoute;
use crate::usage::ToolUseDiagnostics;
use axum::body::Bytes;
use std::sync::Arc;

pub(super) enum OpenedStream {
    Continue(StreamRetryState),
    Respond(Response<Body>),
}

/// Values owned by the attempt loop that a failed stream bootstrap must hand
/// back. A successful stream consumes them.
pub(super) struct StreamRetryState {
    pub(super) request: Value,
    pub(super) request_id: String,
    pub(super) requested_model: String,
    pub(super) prompt_affinity_key: Option<String>,
}

pub(super) struct OpenStreamInput<'a> {
    pub(super) discovery:
        Option<super::super::basis_points::discovery::execution::StreamingDiscovery>,
    pub(super) upstream: reqwest::Response,
    pub(super) status: StatusCode,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) runtime: &'a Arc<GatewayRuntime>,
    pub(super) route: ExecutorRoute,
    pub(super) lease: CandidateLease,
    pub(super) adapter_request: PreparedAdapterRequest,
    pub(super) basis_points_request: Option<super::super::basis_points::PreparedClientRequest>,
    pub(super) request: Value,
    pub(super) request_id: String,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) requested_model: String,
    pub(super) source_model: String,
    pub(super) prompt_affinity_key: Option<String>,
    pub(super) client_wire_api: WireApi,
    pub(super) reasoning_effort: ReasoningEffortDiagnostics,
    pub(super) tool_use: ToolUseDiagnostics,
    pub(super) attempt: u16,
    pub(super) started: Instant,
    pub(super) forwarded_headers: &'a HeaderMap,
    pub(super) adapter_is_passthrough: bool,
    pub(super) has_previous_response_id: bool,
    pub(super) response_affinity_hit: bool,
    pub(super) stream: bool,
    pub(super) resolved_model: &'a str,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) has_unpaired_tool_output: &'a mut bool,
    pub(super) requires_affinity_owner: &'a mut bool,
    pub(super) response_affinity_key: &'a mut Option<String>,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_failure_origin: &'a mut ErrorOrigin,
    pub(super) last_preserved_upstream_error: &'a mut Option<PreservedUpstreamError>,
}

/// Open a client SSE response, or repair a bootstrap failure before any output
/// bytes are forwarded.
pub(super) async fn open_response_stream(stream_input: OpenStreamInput<'_>) -> OpenedStream {
    let OpenStreamInput {
        discovery,
        upstream,
        status,
        response_headers,
        runtime,
        route,
        lease,
        adapter_request,
        basis_points_request,
        mut request,
        request_id,
        key,
        requested_model,
        source_model,
        prompt_affinity_key,
        client_wire_api,
        reasoning_effort,
        tool_use,
        attempt,
        started,
        forwarded_headers,
        adapter_is_passthrough,
        has_previous_response_id,
        response_affinity_hit,
        stream,
        resolved_model,
        selected_error_origin,
        repairs,
        tried,
        has_unpaired_tool_output,
        requires_affinity_owner,
        response_affinity_key,
        last_failure,
        last_failure_origin,
        last_preserved_upstream_error,
    } = stream_input;
    let legacy_call_id_repair_attempted = &mut repairs.legacy_call_id;
    let native_replay_attempted = &mut repairs.native_replay;
    let stale_tool_history_recovered = &mut repairs.stale_tool_history;
    let retry = |request, request_id, requested_model, prompt_affinity_key| StreamRetryState {
        request,
        request_id,
        requested_model,
        prompt_affinity_key,
    };
    let expected_model = (route.account_id.is_some() && runtime.block_degraded_routes_enabled())
        .then_some(route.source_model.as_str());
    let opened = if basis_points_request.is_some() {
        crate::gateway::streaming::bootstrap_basis_points_stream(upstream, expected_model).await
    } else if route.account_transport == crate::runtime::AccountTransport::Siwc
        && adapter_request
            .upstream_body()
            .pointer("/prompt_cache_options/prewarm")
            == Some(&Value::Bool(true))
    {
        // A requested warmup deliberately has no generated output. Do not
        // misclassify its successful terminal as a broken generation stream.
        crate::gateway::streaming::bootstrap_prewarm_stream(upstream, expected_model).await
    } else {
        bootstrap_stream(upstream, expected_model).await
    };
    match opened {
        Ok((headers, first, remaining)) => respond_opened_stream(
            StreamExecution {
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
                client_wire_api,
                reasoning_effort,
                tool_use,
                attempt,
                started,
            },
            status,
            forwarded_headers,
            headers,
            first,
            remaining,
            basis_points_request,
            discovery,
        ),
        Err(bootstrap_failure) => {
            let invalid_function_call_output_call_id =
                bootstrap_failure.invalid_function_call_output_call_id;
            let missing_tool_output = bootstrap_failure
                .preserved
                .as_ref()
                .is_some_and(|error| responses_tool_call_is_missing_output_message(&error.message));
            let tool_links_rejected = bootstrap_failure.responses_tool_call_links_rejected;
            let mut failure = bootstrap_failure
                .failure
                .with_retry_headers(response_headers);
            failure.execution = bootstrap_failure.execution;
            *last_preserved_upstream_error = bootstrap_failure.preserved;
            let upstream_error = bootstrap_failure.upstream_error;
            let mut event = usage_event(
                UsageAttempt {
                    request_id: &request_id,
                    attempt,
                    local_key_id: &key.id,
                    route: &route,
                    reasoning_effort: Some(&reasoning_effort),
                    requested_model: &requested_model,
                    tool_use: tool_use.clone(),
                },
                false,
                failure.status.as_u16(),
                Some(failure.category.to_string()),
                started.elapsed().as_millis() as u64,
            );
            populate_tokens(&mut event, &bootstrap_failure.partial_body);
            event.upstream_error = upstream_error.map(|mut details| {
                details.http_status = Some(status.as_u16());
                details
            });
            let safe_to_repair = failure.execution.certainty == ExecutionCertainty::NotSent
                && failure.category != error_codes::UPSTREAM_TURN_RATE_LIMITED;
            if safe_to_repair
                && try_repair_legacy_responses_call_ids(LegacyCallIdRepair {
                    request: &mut request,
                    client_wire_api,
                    adapter_is_passthrough,
                    upstream_rejected_tool_links: tool_links_rejected,
                    repair_attempted: legacy_call_id_repair_attempted,
                    tried,
                    candidate_id: &route.candidate_id,
                    has_unpaired_tool_output,
                    requires_affinity_owner,
                })
            {
                lease.settle_rotation_repair(now_ms());
                emit_repair(runtime, &mut event);
                return OpenedStream::Continue(retry(
                    request,
                    request_id,
                    requested_model,
                    prompt_affinity_key,
                ));
            }
            if safe_to_repair
                && client_wire_api == WireApi::Responses
                && adapter_is_passthrough
                && has_previous_response_id
                && !*native_replay_attempted
                && ((contains_tool_call_output(&request) && invalid_function_call_output_call_id)
                    || (response_affinity_hit
                        && failure.category == error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND))
            {
                match replay_native_tool_continuation(
                    runtime,
                    &key.id,
                    &mut request,
                    &route,
                    stream,
                    native_replay_attempted,
                ) {
                    Ok(true) => {
                        if failure.category == error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND {
                            event.error_category =
                                Some(error_codes::RESPONSE_AFFINITY_MISS.to_string());
                        }
                        return continue_after_history_repair(
                            runtime,
                            &lease,
                            &route.candidate_id,
                            response_affinity_key,
                            requires_affinity_owner,
                            has_unpaired_tool_output,
                            tried,
                            event,
                            failure,
                            last_failure,
                            last_failure_origin,
                            selected_error_origin,
                            failure.category == error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND,
                            retry(request, request_id, requested_model, prompt_affinity_key),
                        );
                    }
                    Ok(false) => {}
                    Err(error) => {
                        emit_usage(runtime, mark_adapter_failure(event, &error));
                        lease.settle_rotation_terminal(now_ms());
                        return OpenedStream::Respond(adapter_error_response(
                            error,
                            Some(&request_id),
                        ));
                    }
                }
            }
            if safe_to_repair
                && client_wire_api == WireApi::Responses
                && has_previous_response_id
                && missing_tool_output
                && last_preserved_upstream_error.as_ref().is_some_and(|error| {
                    recover_stale_tool_history(
                        runtime,
                        &key.id,
                        &mut request,
                        resolved_model,
                        error.message.as_bytes(),
                        stale_tool_history_recovered,
                    )
                })
            {
                return continue_after_history_repair(
                    runtime,
                    &lease,
                    &route.candidate_id,
                    response_affinity_key,
                    requires_affinity_owner,
                    has_unpaired_tool_output,
                    tried,
                    event,
                    failure,
                    last_failure,
                    last_failure_origin,
                    selected_error_origin,
                    false,
                    retry(request, request_id, requested_model, prompt_affinity_key),
                );
            }
            let failure_state =
                settle_route_failure(runtime, &lease, &route, &failure, response_headers);
            apply_failure_state(&mut event, failure_state);
            let terminal = failure_category_is_request_terminal(failure.category)
                || route_forbids_fallback(&route, failure.status, failure.category)
                || failure.execution.certainty != ExecutionCertainty::NotSent;
            if let Some(routing) = event.routing.as_mut() {
                routing.attempt_outcome = Some(if terminal {
                    crate::AttemptOutcome::ReturnedFailure
                } else {
                    crate::AttemptOutcome::RetrySelection
                });
            }
            emit_usage(runtime, event);
            if terminal {
                return OpenedStream::Respond(attempt_error_response(
                    failure,
                    last_preserved_upstream_error.as_ref(),
                    selected_error_origin,
                    &request_id,
                ));
            }
            *last_failure = Some(failure);
            *last_failure_origin = selected_error_origin;
            OpenedStream::Continue(retry(
                request,
                request_id,
                requested_model,
                prompt_affinity_key,
            ))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn respond_opened_stream(
    execution: StreamExecution,
    status: StatusCode,
    forwarded_headers: &HeaderMap,
    headers: reqwest::header::HeaderMap,
    first: Bytes,
    remaining: UpstreamStream,
    basis_points_request: Option<super::super::basis_points::PreparedClientRequest>,
    discovery: Option<super::super::basis_points::discovery::execution::StreamingDiscovery>,
) -> OpenedStream {
    let account_route = execution.route.account_id.is_some();
    let mut stream_response = if basis_points_request.is_some() || discovery.is_some() {
        execution.into_response_with_basis_points(
            status,
            headers.clone(),
            first,
            remaining,
            basis_points_request,
            discovery,
        )
    } else {
        execution.into_response(status, headers.clone(), first, remaining)
    };
    if account_route {
        relay_account_response_header(forwarded_headers, &headers, &mut stream_response);
    }
    OpenedStream::Respond(stream_response)
}

#[allow(clippy::too_many_arguments)]
fn continue_after_history_repair(
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    candidate_id: &str,
    response_affinity_key: &mut Option<String>,
    requires_affinity_owner: &mut bool,
    has_unpaired_tool_output: &mut bool,
    tried: &mut HashSet<String>,
    mut event: UsageEvent,
    failure: AttemptFailure,
    last_failure: &mut Option<AttemptFailure>,
    last_failure_origin: &mut ErrorOrigin,
    selected_error_origin: ErrorOrigin,
    retain_owner: bool,
    retry: StreamRetryState,
) -> OpenedStream {
    if retain_owner {
        retain_materialized_continuation_owner(requires_affinity_owner, has_unpaired_tool_output);
    } else {
        clear_materialized_continuation(
            response_affinity_key,
            requires_affinity_owner,
            has_unpaired_tool_output,
        );
    }
    tried.remove(candidate_id);
    lease.allow_rotation_repair();
    emit_repair(runtime, &mut event);
    *last_failure = Some(failure);
    *last_failure_origin = selected_error_origin;
    lease.settle_rotation_repair(now_ms());
    OpenedStream::Continue(retry)
}
