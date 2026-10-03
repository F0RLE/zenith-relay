use super::super::super::now_ms;
use super::super::super::response::{
    emit_usage, populate_tokens, proxy_response, proxy_sse_response, usage_event, UsageAttempt,
};
use super::super::super::turn_state::relay_account_response_header;
use super::super::bind_responses_turn;
use super::super::request::{
    adapter_error_response_for_origin, basis_points_relay_error_response,
    handle_basis_points_relay_retry, mark_adapter_failure, BasisPointsRelayRetryContext,
};
use crate::runtime::{AuthenticatedKey, CandidateLease, ExecutorRoute};
use crate::usage::{ReasoningEffortDiagnostics, ToolUseDiagnostics};
use crate::{ErrorOrigin, GatewayRuntime};
use axum::body::Body;
use axum::http::{HeaderMap, Response, StatusCode};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Instant;

pub(super) enum AccountSuccess {
    Continue,
    Respond(Response<Body>),
}

pub(super) struct AccountSuccessInput<'a> {
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
    pub(super) tried: &'a mut HashSet<String>,
    pub(super) basis_points_relay_retry_attempted: &'a mut bool,
    pub(super) basis_points_relay_retry_parameter: &'a mut Option<&'static str>,
    pub(super) last_adapter_error: &'a mut Option<crate::protocol::AdapterError>,
}

/// Record a successful account response and build the client body. A Basis
/// Points translation retry stays inside the attempt loop.
pub(super) fn complete_account_response(input: AccountSuccessInput<'_>) -> AccountSuccess {
    let AccountSuccessInput {
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
        tried,
        basis_points_relay_retry_attempted,
        basis_points_relay_retry_parameter,
        last_adapter_error,
    } = input;
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
    if runtime.block_degraded_routes_enabled() {
        if let Err(rejected) = super::super::super::response::completed_upstream_response(
            &bytes,
            false,
            Some(&route.source_model),
        ) {
            let mut failure = rejected.failure;
            // This account-only endpoint has already collected a full response.
            // Reject the result without starting another generation.
            failure.execution = crate::scheduler::rotation::ExecutionObservation::accepted();
            event.success = false;
            event.http_status = failure.status.as_u16();
            event.error_category = Some(failure.category.to_string());
            let state = super::super::super::errors::settle_attempt_failure(
                runtime,
                lease,
                &route.source_model,
                &failure,
                response_headers,
            );
            super::super::super::errors::apply_failure_state(&mut event, state);
            emit_usage(runtime, event);
            return AccountSuccess::Respond(super::super::attempt_error_response(
                failure,
                rejected.preserved.as_ref(),
                selected_error_origin,
                request_id,
            ));
        }
    }
    let client_bytes = if basis_points_route {
        match super::super::basis_points::translate_response(&bytes, request) {
            Ok(bytes) => bytes,
            Err(error) => {
                return match handle_basis_points_relay_retry(
                    error,
                    &bytes,
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
                    Ok(()) => AccountSuccess::Continue,
                    Err(pair) => {
                        let (error, event) = *pair;
                        AccountSuccess::Respond(basis_points_relay_error_response(
                            error,
                            event,
                            runtime,
                            lease,
                            selected_error_origin,
                        ))
                    }
                };
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
                return AccountSuccess::Respond(adapter_error_response_for_origin(
                    error,
                    selected_error_origin,
                ));
            }
        }
    } else {
        None
    };
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
    bind_responses_turn(
        runtime,
        &key.id,
        &route.candidate_id,
        request,
        &route.source_model,
        &client_bytes,
        true,
    );
    emit_usage(runtime, event);
    lease.settle_rotation_success(now_ms());
    if let Some(stream_body) = basis_points_stream {
        let mut response = proxy_sse_response(status, response_headers, Body::from(stream_body));
        relay_account_response_header(client_headers, response_headers, &mut response);
        return AccountSuccess::Respond(response);
    }
    let mut response = proxy_response(status, response_headers, Body::from(client_bytes));
    if route.account_id.is_some() {
        relay_account_response_header(client_headers, response_headers, &mut response);
    }
    AccountSuccess::Respond(response)
}
