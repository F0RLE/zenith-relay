use super::super::super::request::ServiceTierPolicy;
use super::completion::{complete_buffered_response, BufferedCompletionInput, CompletionStep};
use super::dispatch::{
    dispatch_request_attempt, DispatchedRequestAttempt, RequestDispatch, RequestDispatchInput,
};
use super::failure::{handle_upstream_failure, FailureStep, RejectionCarry, UpstreamFailureInput};
use super::prelude::*;
use super::prepare::{
    prepare_request_attempt, PreparedRequestAttempt, RequestPrepare, RequestPrepareInput,
};
use super::stream::{open_response_stream, OpenStreamInput, OpenedStream};
use std::sync::Arc;

pub(super) enum DrivenAttempt {
    Continue(AttemptCarry),
    Break(AttemptCarry),
    Respond(Response<Body>),
}

/// Request identity owned by the attempt loop. A completed response consumes
/// it; every other step hands it back so the caller can retry or settle.
pub(super) struct AttemptCarry {
    pub(super) request: Value,
    pub(super) request_id: String,
    pub(super) requested_model: String,
    pub(super) prompt_affinity_key: Option<String>,
}

pub(super) struct DriveAttemptInput<'a> {
    pub(super) selected: crate::Selection,
    pub(super) lease: CandidateLease,
    pub(super) request: Value,
    pub(super) request_id: String,
    pub(super) requested_model: String,
    pub(super) prompt_affinity_key: Option<String>,
    pub(super) runtime: &'a Arc<GatewayRuntime>,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) budget: &'a SharedRequestBudget,
    pub(super) transport: crate::UsageTransport,
    pub(super) resolved_model: &'a str,
    pub(super) client_wire_api: WireApi,
    pub(super) stream: bool,
    pub(super) responses_lite: &'a Option<HeaderValue>,
    pub(super) automatic_responses_lite: bool,
    pub(super) service_tier_policy: &'a ServiceTierPolicy,
    pub(super) tool_policy: &'a mut RequestToolPolicy,
    pub(super) client_context_id: &'a Option<String>,
    pub(super) basis_points_relay_retry_parameter: &'a mut Option<&'static str>,
    pub(super) last_adapter_error: &'a mut Option<AdapterError>,
    pub(super) forwarded_headers: &'a HeaderMap,
    pub(super) attempt: &'a mut u16,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_failure_origin: &'a mut ErrorOrigin,
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) has_previous_response_id: bool,
    pub(super) has_unpaired_tool_output: &'a mut bool,
    pub(super) requires_affinity_owner: &'a mut bool,
    pub(super) repairs: &'a mut AttemptRepairs,
    pub(super) last_preserved_upstream_error: &'a mut Option<PreservedUpstreamError>,
    pub(super) response_affinity_key: &'a mut Option<String>,
    pub(super) allow_previous_response_reset: bool,
    pub(super) confirmed_response_missing: &'a mut bool,
    pub(super) basis_points_relay_retry_attempted: &'a mut bool,
}

fn carry(
    request: Value,
    request_id: String,
    requested_model: String,
    prompt_affinity_key: Option<String>,
) -> AttemptCarry {
    AttemptCarry {
        request,
        request_id,
        requested_model,
        prompt_affinity_key,
    }
}

/// Drive one reserved candidate from preparation through a client response.
/// Continue and break return the same request identity the loop still owns.
pub(super) async fn drive_selected_attempt(input: DriveAttemptInput<'_>) -> DrivenAttempt {
    let DriveAttemptInput {
        selected,
        lease,
        mut request,
        request_id,
        requested_model,
        prompt_affinity_key,
        runtime,
        key,
        budget,
        transport,
        resolved_model,
        client_wire_api,
        stream,
        responses_lite,
        automatic_responses_lite,
        service_tier_policy,
        tool_policy,
        client_context_id,
        basis_points_relay_retry_parameter,
        last_adapter_error,
        forwarded_headers,
        attempt,
        last_failure,
        last_failure_origin,
        tried,
        has_previous_response_id,
        has_unpaired_tool_output,
        requires_affinity_owner,
        repairs,
        last_preserved_upstream_error,
        response_affinity_key,
        allow_previous_response_reset,
        confirmed_response_missing,
        basis_points_relay_retry_attempted,
    } = input;
    tried.insert(selected.candidate_id.clone());
    let response_affinity_hit = selected.response_affinity_hit;
    let prepared = match prepare_request_attempt(RequestPrepareInput {
        runtime,
        key,
        request: &mut request,
        resolved_model,
        request_id: &request_id,
        client_wire_api,
        stream,
        responses_lite,
        automatic_responses_lite,
        service_tier_policy,
        tool_policy,
        candidate_id: &selected.candidate_id,
        half_open_probe: selected.half_open_probe,
        diagnostics: selected.diagnostics,
        client_context_id,
        client_transport: transport,
        basis_points_relay_retry_parameter: *basis_points_relay_retry_parameter,
        last_adapter_error,
    }) {
        RequestPrepare::Continue => {
            return DrivenAttempt::Continue(carry(
                request,
                request_id,
                requested_model,
                prompt_affinity_key,
            ));
        }
        RequestPrepare::Respond(response) => return DrivenAttempt::Respond(response),
        RequestPrepare::Ready(prepared) => *prepared,
    };
    let PreparedRequestAttempt {
        route,
        source_model,
        selected_error_origin,
        account_route,
        basis_points_route,
        route_responses_lite,
        client_stream,
        stream,
        summarize,
        adapter_request,
        basis_points_request,
        reasoning_effort,
        adapter_is_passthrough,
        request_body,
        tool_use,
    } = prepared;
    let dispatched = match dispatch_request_attempt(RequestDispatchInput {
        runtime,
        key,
        lease: &lease,
        budget,
        route,
        client_wire_api,
        stream,
        account_route,
        basis_points_route,
        route_responses_lite,
        adapter_request: &adapter_request,
        request_body,
        reasoning_effort: &reasoning_effort,
        tool_use: &tool_use,
        source_model: &source_model,
        request_id: &request_id,
        requested_model: &requested_model,
        forwarded_headers,
        selected_error_origin,
        attempt,
        last_failure,
        last_failure_origin,
    })
    .await
    {
        RequestDispatch::Continue => {
            return DrivenAttempt::Continue(carry(
                request,
                request_id,
                requested_model,
                prompt_affinity_key,
            ));
        }
        RequestDispatch::Respond(response) => return DrivenAttempt::Respond(response),
        RequestDispatch::Ready(dispatched) => *dispatched,
    };
    let DispatchedRequestAttempt {
        route,
        upstream,
        status,
        response_headers,
        started,
    } = dispatched;
    if !status.is_success() {
        match handle_upstream_failure(UpstreamFailureInput {
            upstream,
            status,
            response_headers: &response_headers,
            runtime,
            lease: &lease,
            route: &route,
            source_model: &source_model,
            request_id: &request_id,
            attempt: *attempt,
            key,
            reasoning_effort: &reasoning_effort,
            requested_model: &requested_model,
            tool_use: &tool_use,
            started,
            carry: RejectionCarry {
                client_wire_api,
                request: &mut request,
                adapter_is_passthrough,
                has_previous_response_id,
                repairs,
                tried,
                has_unpaired_tool_output,
                requires_affinity_owner,
                last_failure,
                last_failure_origin,
                last_preserved_upstream_error,
                tool_policy,
                stream,
                response_affinity_key,
                resolved_model,
                allow_previous_response_reset,
                response_affinity_hit,
                selected_error_origin,
                prompt_affinity_key: &prompt_affinity_key,
                confirmed_response_missing,
                account_route,
                forwarded_headers,
            },
        })
        .await
        {
            FailureStep::Continue => {
                return DrivenAttempt::Continue(carry(
                    request,
                    request_id,
                    requested_model,
                    prompt_affinity_key,
                ));
            }
            FailureStep::Break => {
                return DrivenAttempt::Break(carry(
                    request,
                    request_id,
                    requested_model,
                    prompt_affinity_key,
                ));
            }
            FailureStep::Respond(response) => return DrivenAttempt::Respond(response),
        }
    }

    // Managed ChatGPT accounts request the upstream Responses stream even
    // for a buffered client request.  Buffer based on the client contract,
    // then normalize either a JSON response or the completed SSE stream
    // into the same response path.
    if !stream || basis_points_route {
        match complete_buffered_response(BufferedCompletionInput {
            upstream,
            runtime,
            lease: &lease,
            route: &route,
            source_model: &source_model,
            request_id: &request_id,
            attempt: *attempt,
            key,
            reasoning_effort: &reasoning_effort,
            requested_model: &requested_model,
            tool_use: &tool_use,
            started,
            status,
            response_headers: &response_headers,
            account_route,
            client_wire_api,
            request: &mut request,
            adapter_is_passthrough,
            repairs,
            requires_affinity_owner,
            has_unpaired_tool_output,
            last_failure,
            last_failure_origin,
            last_preserved_upstream_error,
            has_previous_response_id,
            basis_points_route,
            basis_points_request: &basis_points_request,
            stream,
            adapter_request,
            basis_points_relay_retry_attempted,
            basis_points_relay_retry_parameter,
            tried,
            last_adapter_error,
            selected_error_origin,
            response_affinity_hit,
            prompt_affinity_key: &prompt_affinity_key,
            summarize,
            client_stream,
            forwarded_headers,
        })
        .await
        {
            CompletionStep::Continue => {
                return DrivenAttempt::Continue(carry(
                    request,
                    request_id,
                    requested_model,
                    prompt_affinity_key,
                ));
            }
            CompletionStep::Respond(response) => return DrivenAttempt::Respond(response),
        }
    }

    match open_response_stream(OpenStreamInput {
        upstream,
        status,
        response_headers: &response_headers,
        runtime,
        route,
        lease,
        adapter_request,
        request,
        request_id,
        key,
        requested_model,
        source_model,
        prompt_affinity_key,
        client_wire_api,
        reasoning_effort,
        tool_use,
        attempt: *attempt,
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
    })
    .await
    {
        OpenedStream::Respond(response) => DrivenAttempt::Respond(response),
        OpenedStream::Continue(kept) => DrivenAttempt::Continue(carry(
            kept.request,
            kept.request_id,
            kept.requested_model,
            kept.prompt_affinity_key,
        )),
    }
}
