use super::super::*;
use super::ConnectTrace;

pub(super) fn record_connect_failure(
    trace: &ConnectTrace<'_>,
    failure: &GatewayFailure,
    headers: Option<&HeaderMap>,
) {
    record_connect_failure_with_hint(trace, failure, headers, RateLimitBodyHint::default());
}

pub(super) fn record_connect_failure_with_hint(
    trace: &ConnectTrace<'_>,
    failure: &GatewayFailure,
    headers: Option<&HeaderMap>,
    hint: RateLimitBodyHint,
) {
    let failure_state = match headers {
        Some(headers) => settle_classified_failure(
            trace.runtime,
            trace.lease,
            &trace.route.source_model,
            failure.status,
            failure.category,
            headers,
            hint,
        ),
        None => settle_classified_failure(
            trace.runtime,
            trace.lease,
            &trace.route.source_model,
            failure.status,
            failure.category,
            &HeaderMap::new(),
            hint,
        ),
    };
    let mut event = connect_usage(
        trace,
        failure.status.as_u16(),
        Some(failure.category.to_string()),
    );
    apply_failure_state(&mut event, failure_state);
    event.upstream_error = failure.upstream_error.as_deref().cloned();
    emit_usage(trace.runtime, event);
}

pub(super) fn record_connect_affinity_miss(trace: &ConnectTrace<'_>, status: StatusCode) {
    emit_usage(
        trace.runtime,
        connect_usage(
            trace,
            status.as_u16(),
            Some(error_codes::RESPONSE_AFFINITY_MISS.to_string()),
        ),
    );
}

pub(super) fn record_connect_rejection(trace: &ConnectTrace<'_>, failure: &GatewayFailure) {
    let mut event = connect_usage(
        trace,
        failure.status.as_u16(),
        Some(failure.category.to_string()),
    );
    event.upstream_error = failure.upstream_error.as_deref().cloned();
    emit_usage(trace.runtime, event);
}

fn connect_usage(
    trace: &ConnectTrace<'_>,
    status: u16,
    category: Option<String>,
) -> super::super::UsageEvent {
    let reasoning = trace.request.reasoning_effort_for(trace.route);
    usage_event(
        UsageAttempt {
            request_id: &trace.request.request_id,
            attempt: trace.attempt,
            local_key_id: &trace.key.id,
            route: trace.route,
            reasoning_effort: Some(&reasoning),
            requested_model: &trace.request.requested_model,
            tool_use: trace.request.tool_use_for(trace.route),
        },
        false,
        status,
        category,
        trace.started.elapsed().as_millis() as u64,
    )
}
