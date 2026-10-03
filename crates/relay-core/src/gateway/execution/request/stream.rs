use super::prelude::*;
use super::recovery::{
    adapter_error_response, recover_stale_tool_history, replay_native_tool_continuation,
    try_repair_legacy_responses_call_ids, LegacyCallIdRepair,
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
    pub(super) upstream: reqwest::Response,
    pub(super) status: StatusCode,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) runtime: &'a Arc<GatewayRuntime>,
    pub(super) route: ExecutorRoute,
    pub(super) lease: CandidateLease,
    pub(super) adapter_request: PreparedAdapterRequest,
    pub(super) request: Value,
    pub(super) request_id: String,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) requested_model: String,
    pub(super) source_model: String,
    pub(super) prompt_affinity_key: Option<String>,
    pub(super) wire_api: WireApi,
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
pub(super) async fn open_response_stream(input: OpenStreamInput<'_>) -> OpenedStream {
    let OpenStreamInput {
        upstream,
        status,
        response_headers,
        runtime,
        route,
        lease,
        adapter_request,
        mut request,
        request_id,
        key,
        requested_model,
        source_model,
        prompt_affinity_key,
        wire_api,
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
    } = input;
    let legacy_call_id_repair_attempted = &mut repairs.legacy_call_id;
    let native_replay_attempted = &mut repairs.native_replay;
    let stale_tool_history_recovered = &mut repairs.stale_tool_history;
    let retry = |request, request_id, requested_model, prompt_affinity_key| StreamRetryState {
        request,
        request_id,
        requested_model,
        prompt_affinity_key,
    };
    match bootstrap_stream(
        upstream,
        (route.account_id.is_some() && runtime.block_degraded_routes_enabled())
            .then_some(route.source_model.as_str()),
    )
    .await
    {
        Ok((headers, first, remaining)) => {
            let account_route = route.account_id.is_some();
            respond_opened_stream(
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
                    wire_api,
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
                account_route,
            )
        }
        Err(bootstrap_failure) => {
            let zenith_gateway_invalid_request = bootstrap_failure.zenith_gateway_invalid_request;
            let missing_tool_output = bootstrap_failure
                .preserved
                .as_ref()
                .is_some_and(|error| responses_tool_call_is_missing_output_message(&error.message));
            let tool_links_rejected = bootstrap_failure.responses_tool_call_links_rejected;
            let mut failure = bootstrap_failure.failure;
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
            event.upstream_error = upstream_error.map(|mut details| {
                details.http_status = Some(status.as_u16());
                details
            });
            let safe_to_repair = failure.execution.certainty == ExecutionCertainty::NotSent;
            if safe_to_repair
                && try_repair_legacy_responses_call_ids(LegacyCallIdRepair {
                    request: &mut request,
                    wire_api,
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
                return OpenedStream::Continue(retry(
                    request,
                    request_id,
                    requested_model,
                    prompt_affinity_key,
                ));
            }
            if safe_to_repair
                && wire_api == WireApi::Responses
                && adapter_is_passthrough
                && has_previous_response_id
                && !*native_replay_attempted
                && ((contains_tool_call_output(&request) && zenith_gateway_invalid_request)
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
                            retry(request, request_id, requested_model, prompt_affinity_key),
                        );
                    }
                    Ok(false) => {}
                    Err(error) => return OpenedStream::Respond(adapter_error_response(error)),
                }
            }
            if safe_to_repair
                && wire_api == WireApi::Responses
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
                    retry(request, request_id, requested_model, prompt_affinity_key),
                );
            }
            let failure_state =
                settle_attempt_failure(runtime, &lease, &source_model, &failure, response_headers);
            apply_failure_state(&mut event, failure_state);
            emit_usage(runtime, event);
            if failure_category_is_request_terminal(failure.category)
                || failure.execution.certainty != ExecutionCertainty::NotSent
            {
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

fn respond_opened_stream(
    execution: StreamExecution,
    status: StatusCode,
    forwarded_headers: &HeaderMap,
    headers: reqwest::header::HeaderMap,
    first: Bytes,
    remaining: UpstreamStream,
    account_route: bool,
) -> OpenedStream {
    let mut response = execution.into_response(status, headers.clone(), first, remaining);
    if account_route {
        relay_account_response_header(forwarded_headers, &headers, &mut response);
    }
    OpenedStream::Respond(response)
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
    event: UsageEvent,
    failure: AttemptFailure,
    last_failure: &mut Option<AttemptFailure>,
    last_failure_origin: &mut ErrorOrigin,
    selected_error_origin: ErrorOrigin,
    retry: StreamRetryState,
) -> OpenedStream {
    clear_materialized_continuation(
        response_affinity_key,
        requires_affinity_owner,
        has_unpaired_tool_output,
    );
    tried.remove(candidate_id);
    lease.allow_rotation_repair();
    emit_usage(runtime, event);
    *last_failure = Some(failure);
    *last_failure_origin = selected_error_origin;
    lease.settle_rotation_repair(now_ms());
    OpenedStream::Continue(retry)
}
