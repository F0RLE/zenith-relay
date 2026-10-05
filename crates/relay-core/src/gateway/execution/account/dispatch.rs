use super::super::super::errors::{
    apply_failure_state, current_failure_state, settle_attempt_failure, AttemptFailure,
};
use super::super::super::now_ms;
use super::super::super::request::{
    apply_codex_routing_hint, codex_client_version, forwarded_codex_headers, AccountEndpoint,
    CODEX_RESPONSES_LITE_HEADER,
};
use super::super::super::response::{emit_usage, usage_event, UsageAttempt};
use super::super::super::turn_state::request_scope;
use super::super::{attempt_error_response, finish_request_failure, RequestFailureInput};
use crate::runtime::{AuthenticatedKey, AuthorizedRequestError, CandidateLease, ExecutorRoute};
use crate::scheduler::rotation::{ExecutionCertainty, RotationOperation, SharedRequestBudget};
use crate::usage::UsageEvent;
use crate::usage::{ReasoningEffortDiagnostics, ToolUseDiagnostics};
use crate::{ErrorOrigin, GatewayRuntime, WireApi};
use axum::body::Body;
use axum::http::header::{ACCEPT, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Response, StatusCode};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Instant;

pub(super) enum AccountDispatch {
    Continue,
    Respond(Response<Body>),
    Ready(Box<DispatchedAccountAttempt>),
}

pub(super) struct DispatchedAccountAttempt {
    pub(super) route: ExecutorRoute,
    pub(super) status: StatusCode,
    pub(super) response_headers: HeaderMap,
    pub(super) bytes: Vec<u8>,
    pub(super) attempt: u16,
    pub(super) started: Instant,
    pub(super) reasoning_effort: ReasoningEffortDiagnostics,
    pub(super) tool_use: ToolUseDiagnostics,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) basis_points_route: bool,
}

pub(super) struct AccountDispatchInput<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) lease: &'a CandidateLease,
    pub(super) budget: &'a SharedRequestBudget,
    pub(super) route: ExecutorRoute,
    pub(super) endpoint: AccountEndpoint,
    pub(super) basis_points_route: bool,
    pub(super) route_responses_lite: Option<HeaderValue>,
    pub(super) selected_error_origin: ErrorOrigin,
    pub(super) upstream_url: url::Url,
    pub(super) upstream_body: Value,
    pub(super) request_body: Vec<u8>,
    pub(super) reasoning_effort: ReasoningEffortDiagnostics,
    pub(super) tool_use: ToolUseDiagnostics,
    pub(super) request_id: &'a str,
    pub(super) requested_model: &'a str,
    pub(super) resolved_model: &'a str,
    pub(super) client_headers: &'a HeaderMap,
    pub(super) operation: RotationOperation,
    pub(super) account_only_exclusions: &'a HashSet<String>,
    pub(super) response_affinity_key: Option<&'a str>,
    pub(super) last_failure: &'a mut Option<AttemptFailure>,
    pub(super) last_failure_origin: &'a mut ErrorOrigin,
}

/// Send one prepared account attempt and collect its body. Transport failures
/// stay retryable until the provider may already have accepted the request.
pub(super) async fn dispatch_account_attempt(input: AccountDispatchInput<'_>) -> AccountDispatch {
    let AccountDispatchInput {
        runtime,
        key,
        lease,
        budget,
        mut route,
        endpoint,
        basis_points_route,
        route_responses_lite,
        selected_error_origin,
        upstream_url,
        upstream_body,
        request_body,
        reasoning_effort,
        tool_use,
        request_id,
        requested_model,
        resolved_model,
        client_headers,
        operation,
        account_only_exclusions,
        response_affinity_key,
        last_failure,
        last_failure_origin,
    } = input;
    let request_body = if basis_points_route {
        match super::super::basis_points::attach_input_images(
            runtime,
            &route.candidate_id,
            &upstream_url,
            &route.upstream_headers,
            request_body,
        )
        .await
        {
            Ok(body) => body,
            Err(super::super::basis_points::AttachmentFailure::Reject(failure)) => {
                return AccountDispatch::Respond(attempt_error_response(
                    failure,
                    None,
                    selected_error_origin,
                    request_id,
                ));
            }
            Err(super::super::basis_points::AttachmentFailure::Retry(failure)) => {
                *last_failure = Some(failure);
                *last_failure_origin = selected_error_origin;
                return AccountDispatch::Continue;
            }
        }
    } else {
        request_body
    };
    let started = Instant::now();
    let failed_usage = |route: &ExecutorRoute, attempt, failure: AttemptFailure| {
        usage_event(
            UsageAttempt {
                request_id,
                attempt,
                local_key_id: &key.id,
                route,
                reasoning_effort: Some(&reasoning_effort),
                requested_model,
                tool_use: tool_use.clone(),
            },
            false,
            failure.status.as_u16(),
            Some(failure.category.to_string()),
            started.elapsed().as_millis() as u64,
        )
    };
    let mut request_headers = if basis_points_route {
        route.upstream_headers.clone()
    } else {
        forwarded_codex_headers(client_headers, request_id)
    };
    if !basis_points_route {
        apply_codex_routing_hint(
            &mut request_headers,
            &route.source_model,
            route.service_tier,
        );
    }
    let turn_account = route.account_id.clone();
    let turn_model = route.source_model.clone();
    let turn_scope = if basis_points_route {
        None
    } else {
        request_scope(
            &key.id,
            client_headers,
            turn_account.as_deref(),
            &turn_model,
        )
    };
    let compaction_headers = request_headers.clone();
    let mut upstream_request = runtime
        .request_client(&route.candidate_id)
        .post(upstream_url)
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json")
        .headers(request_headers);
    if endpoint == AccountEndpoint::Compact && !basis_points_route {
        if let Some(value) = route_responses_lite.as_ref() {
            upstream_request = upstream_request.header(CODEX_RESPONSES_LITE_HEADER, value.clone());
        }
    }
    let upstream = runtime
        .send_authorized_request(
            &route.candidate_id,
            upstream_request.body(request_body),
            (!basis_points_route)
                .then(|| codex_client_version(client_headers))
                .flatten(),
            turn_scope.as_ref(),
            Some(budget),
            Some(lease),
        )
        .await;
    let attempt = u16::from(budget.dispatches());
    let upstream = match upstream {
        Ok(upstream) => {
            route.account_token_generation = upstream.account_token_generation;
            upstream.response
        }
        Err(error) => {
            return reject_authorized_dispatch(
                error,
                runtime,
                lease,
                &route,
                attempt,
                &failed_usage,
                selected_error_origin,
                request_id,
                last_failure,
                last_failure_origin,
            );
        }
    };
    let mut status = upstream.status();
    let mut response_headers = upstream.headers().clone();
    let mut bytes =
        match crate::transport::collect(upstream).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return continue_after_unreadable_body(
                    runtime,
                    lease,
                    &route,
                    attempt,
                    &failed_usage,
                    selected_error_origin,
                    last_failure,
                    last_failure_origin,
                );
            }
        };
    if endpoint == AccountEndpoint::Compact
        && budget.can_dispatch()
        && super::super::super::compaction::missing_legacy_endpoint(status, &bytes)
    {
        match fallback_missing_compact(
            runtime,
            key,
            lease,
            budget,
            &mut route,
            &upstream_body,
            &compaction_headers,
            turn_scope.as_ref(),
            &failed_usage,
            resolved_model,
            operation,
            account_only_exclusions,
            response_affinity_key,
            selected_error_origin,
            request_id,
        )
        .await
        {
            Ok((headers, body)) => {
                status = StatusCode::OK;
                response_headers = headers;
                bytes = body;
            }
            Err(step) => return step,
        }
    }
    AccountDispatch::Ready(Box::new(DispatchedAccountAttempt {
        route,
        status,
        response_headers,
        bytes,
        attempt: u16::from(budget.dispatches()),
        started,
        reasoning_effort,
        tool_use,
        selected_error_origin,
        basis_points_route,
    }))
}

#[allow(clippy::too_many_arguments)]
fn reject_authorized_dispatch(
    error: AuthorizedRequestError,
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    route: &ExecutorRoute,
    attempt: u16,
    failed_usage: &impl Fn(&ExecutorRoute, u16, AttemptFailure) -> UsageEvent,
    selected_error_origin: ErrorOrigin,
    request_id: &str,
    last_failure: &mut Option<AttemptFailure>,
    last_failure_origin: &mut ErrorOrigin,
) -> AccountDispatch {
    let uncertain = error.execution_certainty() == ExecutionCertainty::Unknown;
    let exhausted = matches!(error, AuthorizedRequestError::DispatchBudgetExhausted);
    let failure = AttemptFailure::authorized_request(error);
    let mut event = failed_usage(route, attempt, failure);
    if uncertain || exhausted {
        if uncertain {
            lease.settle_rotation_unknown(now_ms());
        }
        emit_usage(runtime, event);
        return AccountDispatch::Respond(attempt_error_response(
            failure,
            None,
            selected_error_origin,
            request_id,
        ));
    }
    let state = settle_attempt_failure(
        runtime,
        lease,
        &route.source_model,
        &failure,
        &HeaderMap::new(),
    );
    apply_failure_state(&mut event, state);
    emit_usage(runtime, event);
    *last_failure = Some(failure);
    *last_failure_origin = selected_error_origin;
    AccountDispatch::Continue
}

#[allow(clippy::too_many_arguments)]
fn continue_after_unreadable_body(
    runtime: &GatewayRuntime,
    lease: &CandidateLease,
    route: &ExecutorRoute,
    attempt: u16,
    failed_usage: &impl Fn(&ExecutorRoute, u16, AttemptFailure) -> UsageEvent,
    selected_error_origin: ErrorOrigin,
    last_failure: &mut Option<AttemptFailure>,
    last_failure_origin: &mut ErrorOrigin,
) -> AccountDispatch {
    lease.settle_rotation_unknown(now_ms());
    let failure = AttemptFailure::body();
    let state = current_failure_state(runtime, &route.candidate_id, &route.source_model);
    let mut event = failed_usage(route, attempt, failure);
    apply_failure_state(&mut event, state);
    emit_usage(runtime, event);
    *last_failure = Some(failure);
    *last_failure_origin = selected_error_origin;
    AccountDispatch::Continue
}

#[allow(clippy::too_many_arguments, clippy::result_large_err)]
async fn fallback_missing_compact(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    lease: &CandidateLease,
    budget: &SharedRequestBudget,
    route: &mut ExecutorRoute,
    upstream_body: &Value,
    compaction_headers: &HeaderMap,
    turn_scope: Option<&crate::runtime::CodexTurnStateScope<'_>>,
    failed_usage: &impl Fn(&ExecutorRoute, u16, AttemptFailure) -> UsageEvent,
    resolved_model: &str,
    operation: RotationOperation,
    account_only_exclusions: &HashSet<String>,
    response_affinity_key: Option<&str>,
    selected_error_origin: ErrorOrigin,
    request_id: &str,
) -> Result<(HeaderMap, Vec<u8>), AccountDispatch> {
    match super::super::super::compaction::execute(
        runtime,
        route,
        upstream_body,
        compaction_headers,
        turn_scope,
        budget,
        lease,
    )
    .await
    {
        Ok((headers, body)) => Ok((headers, body)),
        Err(error) => {
            let (failure, headers) = *error;
            let mut event = failed_usage(route, u16::from(budget.dispatches()), failure);
            let state =
                settle_attempt_failure(runtime, lease, &route.source_model, &failure, &headers);
            apply_failure_state(&mut event, state);
            emit_usage(runtime, event);
            Err(AccountDispatch::Respond(finish_request_failure(
                RequestFailureInput {
                    runtime,
                    key,
                    resolved_model,
                    protocols: &[WireApi::Responses],
                    operation,
                    exclusions: account_only_exclusions,
                    response_affinity_key,
                    failure,
                    preserved: None,
                    failure_origin: selected_error_origin,
                    request_id,
                },
            )))
        }
    }
}
