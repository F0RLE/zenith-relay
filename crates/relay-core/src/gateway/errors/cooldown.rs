use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_failure_cooldown_with_body(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    status: StatusCode,
    category: &str,
    headers: &reqwest::header::HeaderMap,
    body: Option<&[u8]>,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
) -> FailureState {
    let hint = body.map(rate_limit_body_hint).unwrap_or_default();
    apply_failure_cooldown_with_hint(
        runtime,
        candidate_id,
        model,
        status,
        category,
        headers,
        hint,
        context,
        half_open_probe,
    )
}

pub(crate) fn apply_attempt_failure_cooldown(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    failure: &AttemptFailure,
    headers: &reqwest::header::HeaderMap,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
) -> FailureState {
    apply_failure_cooldown_with_hint(
        runtime,
        candidate_id,
        model,
        failure.status,
        failure.category,
        headers,
        failure.cooldown_hint,
        context,
        half_open_probe,
    )
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct RateLimitBodyHint {
    pub(crate) retry_after_ms: Option<u64>,
    pub(crate) global: bool,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_status_cooldown_with_hint(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    status: StatusCode,
    category: &str,
    headers: &reqwest::header::HeaderMap,
    hint: RateLimitBodyHint,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
) -> FailureState {
    let consecutive_failures = runtime.record_failure(candidate_id);
    let now_system = SystemTime::now();
    let now = crate::unix_time_ms_at(now_system);
    let header_retry_after_ms = retry_after_ms(headers, now_system);
    let has_explicit_retry_after = header_retry_after_ms.is_some() || hint.retry_after_ms.is_some();
    let (scope, automatic_duration_ms) = match status {
        StatusCode::UNAUTHORIZED | StatusCode::PAYMENT_REQUIRED | StatusCode::FORBIDDEN => {
            ("*", 30 * 60_000)
        }
        StatusCode::NOT_FOUND => (model, TRANSIENT_COOLDOWN_MS),
        StatusCode::TOO_MANY_REQUESTS => {
            let duration_ms = rate_limit_cooldown_ms(
                header_retry_after_ms,
                hint.retry_after_ms,
                consecutive_failures,
            );
            // An explicit quota-exhausted classification is account-wide even
            // when the upstream body omits a machine-readable global marker.
            // OpenAI OAuth 429 responses can carry only a reset signal (or a
            // quota phrase in the message), so keeping this model-scoped would
            // let the same exhausted account be selected for another model.
            (rate_limit_scope(category, hint.global, model), duration_ms)
        }
        _ => ("*", TRANSIENT_COOLDOWN_MS),
    };
    let duration_ms = source_cooldown_ms(
        automatic_duration_ms,
        runtime.source_recovery_delay_ms(candidate_id),
        has_explicit_retry_after,
    );
    let duration_ms = half_open_backoff_ms(duration_ms, consecutive_failures, half_open_probe);
    let retry_at_ms = now.saturating_add(duration_ms);
    let reason = failure_cooldown_reason(status, category, has_explicit_retry_after);
    let applied = runtime.set_cooldown_with_reason_for_model_at(
        candidate_id,
        CooldownRequest {
            scope,
            policy_model: model,
            allowed_protocols: context.allowed_protocols,
            request_scope: context.scope,
            retry_at_ms,
            reason,
            now_ms: now,
        },
    );
    FailureState {
        cooldown_scope: applied.then(|| scope.to_string()),
        retry_at_ms: applied.then_some(retry_at_ms),
        consecutive_failures,
    }
}

pub(super) fn rate_limit_scope<'a>(category: &str, global_hint: bool, model: &'a str) -> &'a str {
    if global_hint || category == "upstream_quota_exhausted" {
        "*"
    } else {
        model
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_failure_cooldown_with_hint(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    model: &str,
    status: StatusCode,
    category: &str,
    headers: &reqwest::header::HeaderMap,
    hint: RateLimitBodyHint,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
) -> FailureState {
    let status = canonical_upstream_status(status, category);
    // Once bytes have started flowing, the response cannot be retried for the
    // current client. Keep the next request away from this exact slot/model,
    // rather than opening a candidate-wide circuit for every model it serves.
    if matches!(
        category,
        "upstream_stream" | "stream_incomplete" | "stream_idle_timeout"
    ) {
        return apply_cooldown(
            runtime,
            candidate_id,
            model,
            TRANSIENT_COOLDOWN_MS,
            context,
            half_open_probe,
        );
    }
    if matches!(
        category,
        "upstream_model_not_found"
            | "upstream_model_unsupported"
            | "upstream_model_capacity"
            | "upstream_overloaded"
    ) {
        let has_explicit_retry_after =
            retry_after_ms(headers, SystemTime::now()).is_some() || hint.retry_after_ms.is_some();
        return apply_cooldown_with_reason(
            runtime,
            candidate_id,
            model,
            TRANSIENT_COOLDOWN_MS,
            context,
            half_open_probe,
            failure_cooldown_reason(status, category, has_explicit_retry_after),
        );
    }
    apply_status_cooldown_with_hint(
        runtime,
        candidate_id,
        model,
        status,
        category,
        headers,
        hint,
        context,
        half_open_probe,
    )
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
            "upstream_unauthorized"
                | "upstream_account_disabled"
                | "upstream_usage_not_included"
                | "upstream_quota_exhausted"
                | "upstream_region_unsupported"
                | "upstream_model_not_found"
                | "upstream_model_unsupported"
                | "upstream_model_capacity"
                | "upstream_websocket_connection_limit"
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

pub(crate) fn rate_limit_cooldown_ms(
    header_delay_ms: Option<u64>,
    body_delay_ms: Option<u64>,
    consecutive_failures: u32,
) -> u64 {
    match (header_delay_ms, body_delay_ms) {
        (Some(header), Some(body)) => header.max(body),
        (Some(header), None) => header,
        (None, Some(body)) => body,
        (None, None) => exponential_backoff_ms(consecutive_failures),
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

pub(crate) fn apply_cooldown(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    scope: &str,
    duration_ms: u64,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
) -> FailureState {
    apply_cooldown_with_reason(
        runtime,
        candidate_id,
        scope,
        duration_ms,
        context,
        half_open_probe,
        CooldownReason::Transient,
    )
}

pub(crate) fn apply_cooldown_for_model(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    scope: &str,
    policy_model: &str,
    duration_ms: u64,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
) -> FailureState {
    apply_cooldown_with_reason_for_model(
        runtime,
        candidate_id,
        scope,
        policy_model,
        duration_ms,
        context,
        half_open_probe,
        CooldownReason::Transient,
    )
}

pub(crate) fn apply_cooldown_with_reason(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    scope: &str,
    duration_ms: u64,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
    reason: CooldownReason,
) -> FailureState {
    apply_cooldown_with_reason_for_model(
        runtime,
        candidate_id,
        scope,
        scope,
        duration_ms,
        context,
        half_open_probe,
        reason,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_cooldown_with_reason_for_model(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    scope: &str,
    policy_model: &str,
    duration_ms: u64,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
    reason: CooldownReason,
) -> FailureState {
    let consecutive_failures = runtime.record_failure(candidate_id);
    let duration_ms = source_cooldown_ms(
        duration_ms,
        runtime.source_recovery_delay_ms(candidate_id),
        false,
    );
    let duration_ms = half_open_backoff_ms(duration_ms, consecutive_failures, half_open_probe);
    let now = now_ms();
    let retry_at_ms = now.saturating_add(duration_ms);
    let applied = runtime.set_cooldown_with_reason_for_model_at(
        candidate_id,
        CooldownRequest {
            scope,
            policy_model,
            allowed_protocols: context.allowed_protocols,
            request_scope: context.scope,
            retry_at_ms,
            reason,
            now_ms: now,
        },
    );
    FailureState {
        cooldown_scope: applied.then(|| scope.to_string()),
        retry_at_ms: applied.then_some(retry_at_ms),
        consecutive_failures,
    }
}

pub(crate) fn apply_mandatory_cooldown(
    runtime: &GatewayRuntime,
    candidate_id: &str,
    scope: &str,
    duration_ms: u64,
    context: &CooldownContext<'_>,
    half_open_probe: bool,
) -> FailureState {
    apply_cooldown_with_reason(
        runtime,
        candidate_id,
        scope,
        duration_ms,
        context,
        half_open_probe,
        CooldownReason::Mandatory,
    )
}

pub(crate) fn apply_failure_state(event: &mut UsageEvent, state: FailureState) {
    event.cooldown_scope = state.cooldown_scope;
    event.retry_at_ms = state.retry_at_ms;
    event.consecutive_failures = Some(state.consecutive_failures);
}

pub(crate) fn retry_after_ms(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<u64> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim();
    let duration_ms = if let Ok(seconds) = value.parse::<u64>() {
        seconds.saturating_mul(1_000)
    } else {
        httpdate::parse_http_date(value)
            .ok()?
            .duration_since(now)
            .ok()?
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    };
    Some(duration_ms.min(MAX_RATE_LIMIT_RETRY_HINT_MS))
}

pub(crate) fn exponential_backoff_ms(consecutive_failures: u32) -> u64 {
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    1_000_u64
        .saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX))
        .min(MAX_RATE_LIMIT_COOLDOWN_MS)
}

pub(crate) fn half_open_backoff_ms(
    duration_ms: u64,
    consecutive_failures: u32,
    half_open_probe: bool,
) -> u64 {
    if !half_open_probe {
        return duration_ms;
    }
    let duration_ms = duration_ms.max(1_000);
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    duration_ms
        .saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX))
        .min(duration_ms.max(MAX_RATE_LIMIT_COOLDOWN_MS))
}
