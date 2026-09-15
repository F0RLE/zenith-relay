mod account;
mod client;
mod request;

pub(super) use account::{execute_account_endpoint, AccountExecution};
pub(super) use client::{execute_client_request, execute_gemini_client_request};

use super::errors::{
    api_error_with_origin, api_error_with_origin_and_category, cooldown_error, AttemptFailure,
    PreservedUpstreamError,
};
use super::now_ms;
use crate::runtime::{AuthenticatedKey, GatewayRuntime};
use crate::ErrorOrigin;
use axum::body::Body;
use axum::http::{Response, StatusCode};
use std::collections::HashSet;
use std::time::Duration;

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
        ) {
            return cooldown_error(
                retry_at,
                Some(&failure),
                reason == crate::scheduler::CooldownReason::RateLimit,
            );
        }
    }
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
}

pub(super) async fn wait_for_candidate_retry(
    context: &CandidateRetryContext<'_>,
    tried: &mut HashSet<String>,
    response_affinity_key: Option<&str>,
    retry_wait_attempt: &mut u32,
    backoff: Duration,
    retry_deadline: Option<tokio::time::Instant>,
) -> bool {
    tried.clear();
    *retry_wait_attempt = retry_wait_attempt.saturating_add(1);
    context
        .runtime
        .wait_for_candidate_availability(
            context.runtime.earliest_retry_at(
                context.key,
                context.resolved_model,
                context.protocols,
                tried,
                response_affinity_key,
                now_ms(),
            ),
            backoff,
            retry_deadline,
        )
        .await
}
