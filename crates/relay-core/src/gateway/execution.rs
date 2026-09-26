mod account;
mod basis_points;
mod client;
mod compatibility;
mod request;

pub(super) use account::{execute_account_endpoint, AccountExecution};
pub(super) use client::RoutedRequestIdentity;
pub(super) use client::{execute_client_request, execute_gemini_client_request};

use super::errors::{
    api_error_with_origin, api_error_with_origin_and_category, cooldown_error, AttemptFailure,
    PreservedUpstreamError,
};
use super::now_ms;
use crate::runtime::{AuthenticatedKey, GatewayRuntime};
use crate::scheduler::rotation::RotationOperation;
use crate::ErrorOrigin;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use std::collections::HashSet;

/// Builds the final response after all pre-output route attempts are exhausted.
/// Account and ordinary client execution use the same cooldown and preserved
/// provider-error policy; keeping it here prevents the two retry loops from
/// drifting apart.
#[allow(clippy::too_many_arguments)]
pub(super) fn finish_request_failure(
    runtime: &GatewayRuntime,
    key: &AuthenticatedKey,
    resolved_model: &str,
    protocols: &[crate::WireApi],
    operation: RotationOperation,
    exclusions: &HashSet<String>,
    response_affinity_key: Option<&str>,
    failure: AttemptFailure,
    preserved: Option<&PreservedUpstreamError>,
    failure_origin: ErrorOrigin,
    request_id: &str,
) -> Response<Body> {
    if failure.status == StatusCode::TOO_MANY_REQUESTS {
        if let Some((retry_at, reason)) = runtime.all_applicable_cooldown(
            key,
            resolved_model,
            protocols,
            exclusions,
            response_affinity_key,
            now_ms(),
            operation,
        ) {
            return cooldown_error(
                retry_at,
                Some(&failure),
                reason == crate::scheduler::CooldownReason::RateLimit,
            );
        }
    }
    attempt_error_response(failure, preserved, failure_origin, request_id)
}

/// Keep the safe upstream error only when it belongs to this exact failure.
/// Terminal JSON, stream bootstrap and exhausted retries share this policy.
pub(super) fn attempt_error_response(
    failure: AttemptFailure,
    preserved: Option<&PreservedUpstreamError>,
    failure_origin: ErrorOrigin,
    request_id: &str,
) -> Response<Body> {
    if let Some(preserved) = preserved.filter(|preserved| {
        preserved.status == failure.status && preserved.category == failure.category
    }) {
        return api_error_with_origin_and_category(
            preserved.status,
            &preserved.message,
            &preserved.code,
            preserved.category,
            failure_origin,
            Some(request_id),
        );
    }
    api_error_with_origin(
        failure.status,
        failure.message,
        failure.category,
        failure_origin,
        Some(request_id),
    )
}

pub(super) struct CandidateRetryContext<'a> {
    pub(super) runtime: &'a GatewayRuntime,
    pub(super) key: &'a AuthenticatedKey,
    pub(super) resolved_model: &'a str,
    pub(super) protocols: &'a [crate::WireApi],
    pub(super) operation: RotationOperation,
    pub(super) exclusions: &'a HashSet<String>,
}

/// Await the sole scheduler's next actionable event without a reservation.
/// Attempt counts and the monotonic window belong to the incoming request,
/// not a second recovery state machine created by each transport driver.
pub(super) async fn wait_for_recovery(
    budget: &crate::scheduler::rotation::SharedRequestBudget,
    context: &CandidateRetryContext<'_>,
    tried: &mut HashSet<String>,
    response_affinity_key: Option<&str>,
) -> bool {
    if !budget.can_dispatch() {
        return false;
    }
    // Only an actionable recovery opens the retry window. A plain no-route
    // result must not start it before the first safe rejection/wait.
    if context
        .runtime
        .recovery_retry_at(
            context.key,
            context.resolved_model,
            context.protocols,
            context.exclusions,
            response_affinity_key,
            now_ms(),
            context.operation,
        )
        .is_none()
    {
        return false;
    }
    let deadline = budget.retry_wait_deadline(context.runtime.route_recovery_window_ms());
    if !context
        .runtime
        .wait_for_recovery_event(
            context.key,
            context.resolved_model,
            context.protocols,
            context.exclusions,
            response_affinity_key,
            context.operation,
            budget,
            Some(deadline),
            false,
        )
        .await
    {
        return false;
    }
    tried.clone_from(context.exclusions);
    budget.begin_recovery_pass();
    true
}

pub(super) async fn wait_for_candidate_retry(
    budget: &crate::scheduler::rotation::SharedRequestBudget,
    context: &CandidateRetryContext<'_>,
    tried: &mut HashSet<String>,
    response_affinity_key: Option<&str>,
) -> bool {
    tried.clone_from(context.exclusions);
    let ready = context
        .runtime
        .wait_for_recovery_event(
            context.key,
            context.resolved_model,
            context.protocols,
            context.exclusions,
            response_affinity_key,
            context.operation,
            budget,
            None,
            true,
        )
        .await;
    if ready {
        budget.begin_recovery_pass();
    }
    ready
}
