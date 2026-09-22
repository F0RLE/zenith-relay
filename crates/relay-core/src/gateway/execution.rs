mod account;
mod client;
mod compatibility;
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
    pub(super) exclusions: &'a HashSet<String>,
}

/// One bounded recovery pass, after every currently usable route was tried.
/// The scheduler owns the pause and the exclusive half-open probe, so other
/// requests cannot bypass it while this request waits without holding a lease.
pub(super) struct AutomaticRecovery {
    deadline: tokio::time::Instant,
    recovering: bool,
}

impl AutomaticRecovery {
    pub(super) fn new() -> Self {
        Self {
            deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            recovering: false,
        }
    }

    pub(super) async fn retry(
        &mut self,
        context: &CandidateRetryContext<'_>,
        tried: &mut HashSet<String>,
        response_affinity_key: Option<&str>,
    ) -> bool {
        if !context.runtime.automatic_recovery_enabled() {
            return false;
        }
        let mut exclusions = context.exclusions.clone();
        if self.recovering {
            exclusions.extend(tried.iter().cloned());
        }
        let Some(retry_at) = context.runtime.recovery_retry_at(
            context.key,
            context.resolved_model,
            context.protocols,
            &exclusions,
            response_affinity_key,
            now_ms(),
        ) else {
            return false;
        };
        if tokio::time::Instant::now() >= self.deadline {
            return false;
        }
        let delay = Duration::from_millis(retry_at.saturating_sub(now_ms()));
        if delay
            > self
                .deadline
                .saturating_duration_since(tokio::time::Instant::now())
        {
            return false;
        }
        tokio::time::sleep(delay).await;
        if !self.recovering {
            tried.clone_from(context.exclusions);
            self.recovering = true;
        }
        true
    }
}

pub(super) async fn wait_for_candidate_retry(
    context: &CandidateRetryContext<'_>,
    tried: &mut HashSet<String>,
    response_affinity_key: Option<&str>,
    retry_wait_attempt: &mut u32,
    backoff: Duration,
    retry_deadline: Option<tokio::time::Instant>,
) -> bool {
    tried.clone_from(context.exclusions);
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
