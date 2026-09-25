use super::*;
use crate::error_codes;

#[allow(clippy::too_many_arguments)]
pub(crate) fn settle_status_failure(
    runtime: &GatewayRuntime,
    lease: &crate::runtime::CandidateLease,
    model: &str,
    status: StatusCode,
    category: &'static str,
    headers: &reqwest::header::HeaderMap,
    body: Option<&[u8]>,
) -> FailureState {
    let hint = body.map(rate_limit_body_hint).unwrap_or_default();
    settle_classified_failure(runtime, lease, model, status, category, headers, hint)
}

pub(crate) fn settle_attempt_failure(
    runtime: &GatewayRuntime,
    lease: &crate::runtime::CandidateLease,
    model: &str,
    failure: &AttemptFailure,
    headers: &reqwest::header::HeaderMap,
) -> FailureState {
    let now = SystemTime::now();
    let cooldown = failure_cooldown(
        runtime,
        lease.candidate_id(),
        model,
        failure.status,
        failure.category,
        headers,
        failure.cooldown_hint,
        now,
    );
    failure.settle_rotation_rejection(runtime, lease, cooldown, crate::unix_time_ms_at(now));
    current_failure_state(runtime, lease.candidate_id(), model)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RateLimitBodyHint {
    pub(crate) retry_after_ms: Option<u64>,
    pub(crate) global: bool,
}

/// Compute a provider-scoped block without mutating admission state. The
/// caller must install it in the same critical section as lease settlement.
#[allow(clippy::too_many_arguments)]
pub(crate) fn failure_cooldown<'a>(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &'a str,
    status: StatusCode,
    category: &str,
    headers: &reqwest::header::HeaderMap,
    hint: RateLimitBodyHint,
    now_system: SystemTime,
) -> Option<CooldownRequest<'a>> {
    if !failure_category_requires_cooldown(category) {
        return None;
    }
    let status = canonical_upstream_status(status, category);
    let now = crate::unix_time_ms_at(now_system);
    let header_retry_after_ms = retry_after_ms(headers, now_system);
    let explicit = header_retry_after_ms.is_some() || hint.retry_after_ms.is_some();
    let reason = failure_cooldown_reason(status, category, explicit);
    if reason == CooldownReason::Transient {
        return None;
    }
    let scope = if status == StatusCode::TOO_MANY_REQUESTS {
        rate_limit_scope(category, hint.global, model)
    } else if matches!(
        status,
        StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN
    ) {
        "*"
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
        || category == error_codes::UPSTREAM_QUOTA_EXHAUSTED
    {
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
    let now = SystemTime::now();
    let cooldown = failure_cooldown(
        runtime,
        lease.candidate_id(),
        model,
        status,
        category,
        headers,
        hint,
        now,
    );
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
                | error_codes::UPSTREAM_MODEL_UNSUPPORTED
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

pub(crate) fn rate_limit_body_hint(body: &[u8]) -> RateLimitBodyHint {
    rate_limit_body_hint_at(body, SystemTime::now())
}

pub(crate) fn rate_limit_body_hint_at(body: &[u8], now: SystemTime) -> RateLimitBodyHint {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return RateLimitBodyHint::default();
    };
    rate_limit_body_hint_value(&value, now)
}

pub(crate) fn rate_limit_body_hint_value(value: &Value, now: SystemTime) -> RateLimitBodyHint {
    let retry_after_ms = rate_limit_reset_delay_ms(value, now)
        .or_else(|| {
            [
                "/resets_in_seconds",
                "/error/resets_in_seconds",
                "/body/error/resets_in_seconds",
                "/response/error/resets_in_seconds",
            ]
            .into_iter()
            .find_map(|path| value.pointer(path).and_then(json_seconds_to_ms))
        })
        .or_else(|| {
            [
                "/retry_after",
                "/error/retry_after",
                "/body/error/retry_after",
                "/response/error/retry_after",
            ]
            .into_iter()
            .find_map(|path| value.pointer(path).and_then(json_seconds_to_ms))
        })
        .or_else(|| retry_delay_from_text(&upstream_error_text(value)));
    let global = [
        "/type",
        "/code",
        "/error/type",
        "/error/code",
        "/body/error/type",
        "/body/error/code",
        "/response/error/type",
        "/response/error/code",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    .map(str::to_ascii_lowercase)
    .any(|kind| {
        kind.contains("usage_limit")
            || kind.contains("usage_not_included")
            || kind.contains("quota")
            || kind.contains("credits_depleted")
            || matches!(
                kind.as_str(),
                "rate_limit_reached" | "websocket_connection_limit_reached"
            )
    });
    RateLimitBodyHint {
        retry_after_ms,
        global,
    }
}

pub(crate) fn rate_limit_reset_delay_ms(value: &Value, now: SystemTime) -> Option<u64> {
    let reset_at = [
        "/resets_at",
        "/error/resets_at",
        "/body/error/resets_at",
        "/response/error/resets_at",
    ]
    .into_iter()
    .find_map(|path| value.pointer(path).and_then(json_u64))?;
    let reset_seconds = if reset_at > 10_000_000_000 {
        reset_at / 1_000
    } else {
        reset_at
    };
    let now_seconds = now.duration_since(UNIX_EPOCH).ok()?.as_secs();
    reset_seconds
        .checked_sub(now_seconds)
        .and_then(|seconds| seconds.checked_mul(1_000))
        .filter(|duration_ms| *duration_ms > 0)
        .map(|duration_ms| duration_ms.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}

pub(crate) fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
}

pub(crate) fn json_seconds_to_ms(value: &Value) -> Option<u64> {
    let seconds = value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))?;
    if !seconds.is_finite() || seconds <= 0.0 {
        return None;
    }
    Some(
        (seconds * 1_000.0)
            .ceil()
            .min(MAX_RATE_LIMIT_RETRY_HINT_MS as f64) as u64,
    )
}

pub(crate) fn retry_delay_from_text(text: &str) -> Option<u64> {
    let suffix = text.split_once("try again in")?.1.trim_start();
    let number_end = suffix
        .find(|character: char| !(character.is_ascii_digit() || character == '.'))
        .unwrap_or(suffix.len());
    let seconds_or_millis = suffix[..number_end].parse::<f64>().ok()?;
    if !seconds_or_millis.is_finite() || seconds_or_millis <= 0.0 {
        return None;
    }
    let unit = suffix[number_end..].trim_start();
    let multiplier = if unit.starts_with("ms") || unit.starts_with("millisecond") {
        1.0
    } else if unit.starts_with('s') || unit.starts_with("second") {
        1_000.0
    } else {
        return None;
    };
    Some(
        (seconds_or_millis * multiplier)
            .ceil()
            .min(MAX_RATE_LIMIT_RETRY_HINT_MS as f64) as u64,
    )
}

pub(crate) fn retry_delay_ms(header: Option<u64>, body: Option<u64>, fallback: u64) -> u64 {
    header.into_iter().chain(body).max().unwrap_or(fallback)
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

pub(crate) fn retry_after_ms(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<u64> {
    crate::transport::retry_after_ms(headers, now)
        .map(|delay| delay.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}
