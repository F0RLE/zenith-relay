use super::super::super::now_ms;
use super::super::super::request::AccountEndpoint;
use super::super::super::response::{
    emit_usage, populate_tokens, proxy_response, proxy_sse_response, usage_event, UsageAttempt,
};
use super::super::super::turn_state::relay_account_response_header;
use super::super::bind_responses_turn;
use super::super::request::{adapter_error_response_for_origin, mark_adapter_failure};
use crate::protocol::completion::ResponseCompletion;
use crate::runtime::{AuthenticatedKey, CandidateLease, ExecutorRoute};
use crate::usage::{ReasoningEffortDiagnostics, ToolUseDiagnostics};
use crate::{ErrorOrigin, GatewayRuntime};
use axum::body::Body;
use axum::http::{HeaderMap, Response, StatusCode};
use serde_json::Value;
use std::time::Instant;

pub(super) struct AccountSuccessInput<'a> {
    pub(super) endpoint: AccountEndpoint,
    pub(super) status: StatusCode,
    pub(super) bytes: Vec<u8>,
    pub(super) response_headers: &'a HeaderMap,
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) lease: &'a CandidateLease,
    pub(super) route: &'a ExecutorRoute,
    pub(super) attempt: u16,
    pub(super) request_id: &'a str,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) reasoning_effort: &'a ReasoningEffortDiagnostics,
    pub(super) requested_model: &'a str,
    pub(super) tool_use: ToolUseDiagnostics,
    pub(super) started: Instant,
    pub(super) request: &'a Value,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) basis_points_route: bool,
    pub(super) prompt_affinity_key: &'a Option<String>,
}

/// Build the client body from a completed generation. Translation failures
/// settle the attempt without repeating accepted execution.
pub(super) fn complete_account_response(
    account_success_input: AccountSuccessInput<'_>,
) -> Response<Body> {
    let AccountSuccessInput {
        endpoint,
        status,
        bytes,
        response_headers,
        runtime,
        lease,
        route,
        attempt,
        request_id,
        key,
        reasoning_effort,
        requested_model,
        tool_use,
        started,
        request,
        client_headers,
        selected_error_origin,
        basis_points_route,
        prompt_affinity_key,
    } = account_success_input;
    let client_stream = request.get("stream").and_then(Value::as_bool) == Some(true);
    let mut event = usage_event(
        UsageAttempt {
            request_id,
            attempt,
            local_key_id: &key.id,
            route,
            reasoning_effort: Some(reasoning_effort),
            requested_model,
            tool_use,
        },
        true,
        status.as_u16(),
        None,
        started.elapsed().as_millis() as u64,
    );
    populate_tokens(&mut event, &bytes);
    let expected_model = runtime
        .block_degraded_routes_enabled()
        .then_some(route.source_model.as_str());
    let (bytes, completion) = match account_response(
        &bytes,
        endpoint,
        expected_model,
        route.account_transport == crate::runtime::AccountTransport::Siwc,
    ) {
        Ok(response) => response,
        Err(rejected) => {
            let mut failure = rejected.failure;
            // This account-only endpoint has already collected a full response.
            // Reject the result without starting another generation.
            failure.execution = crate::scheduler::rotation::ExecutionObservation::accepted();
            event.success = false;
            event.http_status = failure.status.as_u16();
            event.error_category = Some(failure.category.to_string());
            let failure_state = super::super::super::errors::settle_route_failure(
                runtime,
                lease,
                route,
                &failure,
                response_headers,
            );
            super::super::super::errors::apply_failure_state(&mut event, failure_state);
            emit_usage(runtime, event);
            return super::super::attempt_error_response(
                failure,
                rejected.preserved.as_ref(),
                selected_error_origin,
                request_id,
            );
        }
    };
    let completed = completion == ResponseCompletion::Completed;
    event.success = completed;
    event.error_category =
        (!completed).then(|| crate::error_codes::RESPONSE_INCOMPLETE.to_string());
    let client_bytes = if basis_points_route {
        match super::super::basis_points::translate_response(&bytes, request) {
            Ok(bytes) => bytes,
            Err(error) => {
                emit_usage(runtime, mark_adapter_failure(event, &error));
                lease.settle_rotation_terminal(now_ms());
                return adapter_error_response_for_origin(
                    error,
                    selected_error_origin,
                    Some(request_id),
                );
            }
        }
    } else {
        bytes
    };
    let basis_points_stream = if basis_points_route && client_stream {
        match super::super::basis_points::synthetic_stream(&client_bytes) {
            Ok(stream_body) => Some(stream_body),
            Err(error) => {
                emit_usage(runtime, mark_adapter_failure(event, &error));
                lease.settle_rotation_terminal(now_ms());
                return adapter_error_response_for_origin(
                    error,
                    selected_error_origin,
                    Some(request_id),
                );
            }
        }
    } else {
        None
    };
    if completed {
        let recovered = lease.settle_rotation_success_with_latency(now_ms(), event.ttft_ms);
        event.consecutive_failures = recovered.then_some(0);
        runtime.bind_prompt_affinity(
            prompt_affinity_key.as_deref(),
            &route.candidate_id,
            now_ms(),
        );
        bind_responses_turn(
            runtime,
            &key.id,
            route,
            request,
            &route.source_model,
            &client_bytes,
            true,
        );
    } else {
        lease.settle_rotation_terminal(now_ms());
    }
    emit_usage(runtime, event);
    if let Some(stream_body) = basis_points_stream {
        let mut response = proxy_sse_response(status, response_headers, Body::from(stream_body));
        relay_account_response_header(client_headers, response_headers, &mut response);
        return response;
    }
    let mut response = proxy_response(status, response_headers, Body::from(client_bytes));
    if route.account_id.is_some() {
        relay_account_response_header(client_headers, response_headers, &mut response);
    }
    response
}

fn account_response(
    bytes: &[u8],
    endpoint: AccountEndpoint,
    expected_model: Option<&str>,
    siwc_route: bool,
) -> Result<
    (Vec<u8>, ResponseCompletion),
    Box<super::super::super::streaming::StreamBootstrapFailure>,
> {
    use super::super::super::errors::AttemptFailure;
    use super::super::super::response::{completed_upstream_response, validate_upstream_response};

    let mut bytes = if matches!(endpoint, AccountEndpoint::Wake)
        || serde_json::from_slice::<Value>(bytes).is_err()
    {
        completed_upstream_response(bytes, true, expected_model)?
    } else {
        bytes.to_vec()
    };
    if siwc_route && endpoint == AccountEndpoint::Compact {
        crate::protocol::wrap_compaction_response_bytes(&mut bytes).map_err(|_| {
            Box::new(AttemptFailure::stream(crate::error_codes::COMPACTION_RESPONSE_INVALID).into())
        })?;
    }
    let body = serde_json::from_slice::<Value>(&bytes)
        .map_err(|_| Box::new(AttemptFailure::stream(crate::error_codes::STREAM_INVALID).into()))?;
    validate_upstream_response(&body, expected_model)?;
    let completion = match endpoint {
        AccountEndpoint::Wake => ResponseCompletion::for_protocol(crate::WireApi::Responses, &body),
        AccountEndpoint::Compact => {
            let compacted = super::super::super::compaction::compaction_document(&body)
                .map_err(|failure| Box::new(failure.into()))?;
            if siwc_route {
                bytes = compacted;
            }
            ResponseCompletion::Completed
        }
        AccountEndpoint::AlphaSearch => ResponseCompletion::Completed,
    };
    Ok((bytes, completion))
}
