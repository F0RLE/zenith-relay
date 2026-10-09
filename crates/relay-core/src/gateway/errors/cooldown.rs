use super::*;
use crate::error_codes;

mod rate_limit;

#[cfg(test)]
pub(crate) use rate_limit::rate_limit_body_hint_at;
pub(crate) use rate_limit::{
    rate_limit_body_hint, rate_limit_body_hint_value, retry_after_ms, retry_delay_ms,
    RateLimitBodyHint,
};

#[allow(clippy::too_many_arguments)]
pub(crate) fn settle_status_failure(
    runtime: &GatewayRuntime,
    lease: &crate::runtime::CandidateLease,
    model: &str,
    status: StatusCode,
    category: &'static str,
    headers: &reqwest::header::HeaderMap,
    response_body: Option<&[u8]>,
) -> FailureState {
    let hint = response_body.map(rate_limit_body_hint).unwrap_or_default();
    settle_classified_failure(runtime, lease, model, status, category, headers, hint)
}

pub(crate) fn settle_attempt_failure(
    runtime: &GatewayRuntime,
    lease: &crate::runtime::CandidateLease,
    model: &str,
    failure: &AttemptFailure,
    headers: &reqwest::header::HeaderMap,
) -> FailureState {
    let category = runtime.effective_upstream_category(failure.category);
    let now = SystemTime::now();
    let cooldown = failure_cooldown(CooldownInput {
        runtime,
        candidate_id: lease.candidate_id(),
        model,
        status: failure.status,
        category,
        headers,
        hint: failure.cooldown_hint,
        now,
    });
    failure.settle_rotation_rejection(runtime, lease, cooldown, crate::unix_time_ms_at(now));
    current_failure_state(runtime, lease.candidate_id(), model)
}

pub(crate) fn settle_route_failure(
    runtime: &GatewayRuntime,
    lease: &crate::runtime::CandidateLease,
    route: &ExecutorRoute,
    failure: &AttemptFailure,
    headers: &reqwest::header::HeaderMap,
) -> FailureState {
    settle_attempt_failure(runtime, lease, &route.source_model, failure, headers)
}

/// Compute a provider-scoped block without mutating admission state. The
/// caller must install it in the same critical section as lease settlement.
pub(crate) struct CooldownInput<'a> {
    pub(crate) runtime: &'a GatewayRuntime,
    pub(crate) candidate_id: &'a str,
    pub(crate) model: &'a str,
    pub(crate) status: StatusCode,
    pub(crate) category: &'a str,
    pub(crate) headers: &'a reqwest::header::HeaderMap,
    pub(crate) hint: RateLimitBodyHint,
    pub(crate) now: SystemTime,
}

pub(crate) fn failure_cooldown(cooldown_input: CooldownInput<'_>) -> Option<CooldownRequest<'_>> {
    let CooldownInput {
        runtime,
        candidate_id,
        model,
        status,
        category,
        headers,
        hint,
        now: now_system,
    } = cooldown_input;
    if !failure_category_requires_cooldown(category) {
        return None;
    }
    let status = canonical_upstream_status(status, category);
    let now = crate::unix_time_ms_at(now_system);
    let header_retry_after_ms = retry_after_ms(headers, now_system);
    let explicit = header_retry_after_ms.is_some()
        || hint.retry_after_ms.is_some()
        || category == error_codes::UPSTREAM_ROUTE_DEGRADED;
    let reason = failure_cooldown_reason(status, category, explicit);
    if reason == CooldownReason::Transient {
        return None;
    }
    let scope = if status == StatusCode::TOO_MANY_REQUESTS {
        rate_limit_scope(category, hint.global, model)
    } else if category.starts_with("upstream_model_")
        || matches!(
            category,
            error_codes::UPSTREAM_WEBSOCKET_CONNECTION_LIMIT
                | error_codes::UPSTREAM_OVERLOADED
                | error_codes::UPSTREAM_CANDIDATE_REJECTED
                | error_codes::IMAGE_GENERATION_NOT_ENABLED
        )
    {
        model
    } else {
        "*"
    };
    let fallback = if scope == "*"
        && matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN
        )
        || matches!(
            category,
            error_codes::UPSTREAM_QUOTA_EXHAUSTED | error_codes::UPSTREAM_ROUTE_DEGRADED
        ) {
        MAX_RATE_LIMIT_COOLDOWN_MS
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        1_000
    } else {
        TRANSIENT_COOLDOWN_MS
    };
    let duration_ms = retry_delay_ms(header_retry_after_ms, hint.retry_after_ms, fallback);
    let retry_at_ms = now.saturating_add(source_cooldown_ms(
        duration_ms,
        runtime.source_recovery_delay_ms(candidate_id),
        explicit,
    ));
    Some(CooldownRequest {
        scope,
        retry_at_ms,
        reason,
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn settle_classified_failure(
    runtime: &GatewayRuntime,
    lease: &crate::runtime::CandidateLease,
    model: &str,
    status: StatusCode,
    category: &'static str,
    headers: &reqwest::header::HeaderMap,
    hint: RateLimitBodyHint,
) -> FailureState {
    let category = runtime.effective_upstream_category(category);
    let now = SystemTime::now();
    let cooldown = failure_cooldown(CooldownInput {
        runtime,
        candidate_id: lease.candidate_id(),
        model,
        status,
        category,
        headers,
        hint,
        now,
    });
    AttemptFailure::classified_with_hint(status, category, hint).settle_rotation_rejection(
        runtime,
        lease,
        cooldown,
        crate::unix_time_ms_at(now),
    );
    current_failure_state(runtime, lease.candidate_id(), model)
}

pub(crate) fn current_failure_state(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
) -> FailureState {
    let (consecutive_failures, cooldown) = runtime.failure_state_for(candidate_id, model, now_ms());
    FailureState {
        consecutive_failures,
        retry_at_ms: cooldown.as_ref().map(|(_, at)| *at),
        cooldown_scope: cooldown.map(|(scope, _)| scope),
    }
}

pub(super) fn rate_limit_scope<'a>(category: &str, global_hint: bool, model: &'a str) -> &'a str {
    if global_hint || category == error_codes::UPSTREAM_QUOTA_EXHAUSTED {
        "*"
    } else {
        model
    }
}

pub(crate) fn failure_cooldown_reason(
    status: StatusCode,
    category: &str,
    explicit_retry_after: bool,
) -> CooldownReason {
    if status == StatusCode::TOO_MANY_REQUESTS {
        return CooldownReason::RateLimit;
    }
    if explicit_retry_after
        || matches!(
            status,
            StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN
        )
        || matches!(
            category,
            error_codes::UPSTREAM_UNAUTHORIZED
                | error_codes::UPSTREAM_ACCOUNT_DISABLED
                | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
                | error_codes::UPSTREAM_QUOTA_EXHAUSTED
                | error_codes::UPSTREAM_REGION_UNSUPPORTED
                | error_codes::UPSTREAM_MODEL_NOT_FOUND
                | error_codes::UPSTREAM_MODEL_UNAVAILABLE
                | error_codes::UPSTREAM_MODEL_UNSUPPORTED
                | error_codes::UPSTREAM_ROUTE_DEGRADED
                | error_codes::UPSTREAM_MODEL_CAPACITY
                | error_codes::UPSTREAM_CANDIDATE_REJECTED
                | error_codes::IMAGE_GENERATION_NOT_ENABLED
                | error_codes::UPSTREAM_WEBSOCKET_CONNECTION_LIMIT
        )
    {
        CooldownReason::Mandatory
    } else {
        CooldownReason::Transient
    }
}

pub(crate) fn source_cooldown_ms(
    automatic_ms: u64,
    configured_ms: Option<u64>,
    explicit_hint: bool,
) -> u64 {
    configured_ms.map_or(automatic_ms, |configured| {
        if explicit_hint {
            automatic_ms.max(configured)
        } else {
            configured
        }
    })
}

pub(crate) fn settle_image_capability_failure(
    runtime: &GatewayRuntime,
    lease: &crate::runtime::CandidateLease,
    scope: &str,
    duration_ms: u64,
) -> FailureState {
    let now = now_ms();
    let retry_at_ms = now.saturating_add(source_cooldown_ms(
        duration_ms,
        runtime.source_recovery_delay_ms(lease.candidate_id()),
        true,
    ));
    AttemptFailure::classified_with_hint(
        StatusCode::BAD_REQUEST,
        error_codes::IMAGE_GENERATION_NOT_ENABLED,
        RateLimitBodyHint::default(),
    )
    .settle_rotation_rejection(
        runtime,
        lease,
        Some(CooldownRequest {
            scope,
            retry_at_ms,
            reason: CooldownReason::Mandatory,
        }),
        now,
    );
    current_failure_state(runtime, lease.candidate_id(), scope)
}

pub(crate) fn apply_failure_state(event: &mut UsageEvent, state: FailureState) {
    event.cooldown_scope = state.cooldown_scope;
    event.retry_at_ms = state.retry_at_ms;
    event.consecutive_failures = Some(state.consecutive_failures);
}
