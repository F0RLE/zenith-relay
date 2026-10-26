use super::super::super::request::ServiceTierPolicy;
use super::super::basis_points::structured_output::requires_buffering;
use super::super::compatibility::{AdapterRejection, record_adapter_rejection};
use super::completion::{BufferedCompletionInput, CompletionStep, complete_buffered_response};
use super::dispatch::{
    DispatchedRequestAttempt, RequestDispatch, RequestDispatchInput, dispatch_request_attempt,
};
use super::failure::{FailureStep, RejectionCarry, UpstreamFailureInput, handle_upstream_failure};
use super::prelude::*;
use super::prepare::{
    PreparedRequestAttempt, RequestPrepare, RequestPrepareInput, prepare_request_attempt,
};
use super::recovery::{adapter_error_response, replay_native_tool_continuation};
use super::state::{AttemptState, ContinuationState, RetryState};
use super::stream::{OpenStreamInput, OpenedStream, open_response_stream};
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
    pub(super) route: crate::runtime::ExecutorRoute,
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
    pub(super) transport_fallback: Option<crate::usage::TransportFallbackReason>,
    pub(super) resolved_model: &'a str,
    pub(super) client_wire_api: WireApi,
    pub(super) stream: bool,
    pub(super) responses_lite: &'a Option<HeaderValue>,
    pub(super) service_tier_policy: &'a ServiceTierPolicy,
    pub(super) tool_policy: &'a mut RequestToolPolicy,
    pub(super) client_context_id: &'a Option<String>,
    pub(super) forwarded_headers: &'a HeaderMap,
    pub(super) state: &'a mut AttemptState,
    pub(super) has_previous_response_id: bool,
    pub(super) allow_previous_response_reset: bool,
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
pub(super) async fn drive_selected_attempt(drive_input: DriveAttemptInput<'_>) -> DrivenAttempt {
    let DriveAttemptInput {
        route,
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
        transport_fallback,
        resolved_model,
        client_wire_api,
        stream,
        responses_lite,
        service_tier_policy,
        tool_policy,
        client_context_id,
        forwarded_headers,
        state,
        mut has_previous_response_id,
        allow_previous_response_reset,
    } = drive_input;
    let AttemptState {
        retry,
        continuation,
    } = state;
    let RetryState {
        attempt,
        tried,
        preparation_exclusions,
        last_failure,
        last_failure_origin,
        last_adapter_error,
        last_preserved_upstream_error,
        window_expired: _,
    } = retry;
    let ContinuationState {
        response_affinity_key,
        requires_affinity_owner,
        has_unpaired_tool_output,
        confirmed_response_missing,
        repairs,
    } = continuation;
    if matches!(
        route.account_transport,
        AccountTransport::ExcelBasisPoints | AccountTransport::Siwc
    ) && route.adapter.is_passthrough()
        && client_wire_api == WireApi::Responses
        && has_previous_response_id
    {
        // BPS and SIWC HTTP have no provider-side response store. Admission
        // has already checked this predecessor's exact owner; materialize
        // its bounded history before preparation, including WS-to-HTTP turns.
        let started = Instant::now();
        let replay = replay_native_tool_continuation(
            runtime,
            &key.id,
            &mut request,
            &route,
            stream,
            &mut repairs.native_replay,
        )
        .and_then(|restored| {
            restored
                .then_some(())
                .ok_or_else(AdapterError::continuation_missing)
        });
        if let Err(error) = replay {
            record_adapter_rejection(
                runtime,
                AdapterRejection {
                    request_id: &request_id,
                    attempt: *attempt,
                    local_key_id: &key.id,
                    requested_model: &requested_model,
                    resolved_model,
                    client_context_id: client_context_id.as_deref(),
                    wire_api: client_wire_api,
                    transport,
                    service_tier: service_tier_policy.selected_tier(),
                },
                &error,
                started.elapsed().as_millis() as u64,
            );
            return DrivenAttempt::Respond(adapter_error_response(error, Some(&request_id)));
        }
        clear_materialized_continuation(
            response_affinity_key,
            requires_affinity_owner,
            has_unpaired_tool_output,
        );
        has_previous_response_id = false;
    }
    tried.insert(selected.candidate_id.clone());
    let response_affinity_hit = selected.response_affinity_hit;
    let preparation_started = route.diagnostics_enabled.then(Instant::now);
    let prepared = match prepare_request_attempt(RequestPrepareInput {
        route,
        runtime,
        key,
        request: &mut request,
        resolved_model,
        request_id: &request_id,
        client_wire_api,
        stream,
        responses_lite,
        service_tier_policy,
        tool_policy,
        candidate_id: &selected.candidate_id,
        half_open_probe: selected.half_open_probe,
        diagnostics: selected.diagnostics,
        client_context_id,
        client_transport: transport,
        last_adapter_error,
    }) {
        RequestPrepare::Continue => {
            preparation_exclusions.insert(selected.candidate_id.clone());
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
        preparation,
        mut route,
        source_model,
        selected_error_origin,
        account_route,
        basis_points_route,
        route_responses_lite,
        client_stream,
        stream,
        summarize,
        mut adapter_request,
        basis_points_request,
        siwc_catalog_request,
        reasoning_effort,
        adapter_is_passthrough,
        request_body,
        mut tool_use,
    } = prepared;
    route.observe_http_dispatch(transport_fallback);
    let dispatches_before = budget.dispatches();
    let dispatched = match dispatch_request_attempt(RequestDispatchInput {
        preparation_started,
        preparation,
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
        adapter_request: &mut adapter_request,
        request_body,
        reasoning_effort: &reasoning_effort,
        tool_use: &mut tool_use,
        basis_points_request: basis_points_request.as_ref(),
        siwc_catalog_request: siwc_catalog_request.as_ref(),
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
            // Uploads and rejected dispatch fences can fail without spending a
            // generation. They must not reopen a ready member on every pass.
            if budget.dispatches() == dispatches_before {
                preparation_exclusions.insert(selected.candidate_id.clone());
            }
            return DrivenAttempt::Continue(carry(
                request,
                request_id,
                requested_model,
                prompt_affinity_key,
            ));
        }
        RequestDispatch::Break => {
            return DrivenAttempt::Break(carry(
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
        discovery,
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
    let upstream_sse = response_headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .is_some_and(|mime| mime.trim().eq_ignore_ascii_case("text/event-stream"))
        });
    // Local JSON validation withholds the whole answer. It must never
    // expose invalid text through incremental deltas before terminal validation.
    let validate_output = basis_points_request
        .as_ref()
        .is_some_and(|request| requires_buffering(request));
    if !stream || (basis_points_route && (!upstream_sse || validate_output)) {
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
            basis_points_request: &basis_points_request,
            stream,
            adapter_request,
            tried,
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
        discovery,
        upstream,
        status,
        response_headers: &response_headers,
        runtime,
        route,
        lease,
        adapter_request,
        basis_points_request,
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
