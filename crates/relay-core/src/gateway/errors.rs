use super::now_ms;
use crate::error_codes;
use crate::runtime::{AuthorizedRequestError, ExecutorPrepareError};
use crate::scheduler::{CooldownReason, CooldownRequest};
use crate::{GatewayRuntime, UsageEvent};
use axum::body::Body;
use axum::http::header::RETRY_AFTER;
use axum::http::{HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};
use std::time::SystemTime;

mod classify;
mod cooldown;
mod failure;
mod response;

pub(super) use classify::{classify_upstream_error, classify_upstream_error_value};
pub(crate) use classify::{is_deactivated_workspace, is_deactivated_workspace_value};
use classify::{normalized_error_text, text_has_any, upstream_error_text};

pub(super) use cooldown::{
    apply_failure_state, current_failure_state, failure_cooldown, rate_limit_body_hint,
    rate_limit_body_hint_value, settle_attempt_failure, settle_classified_failure,
    settle_image_capability_failure, settle_status_failure, CooldownInput, RateLimitBodyHint,
};

pub(crate) use failure::failure_category_affects_account_state;
pub(super) use failure::{
    failure_category_is_request_terminal, failure_category_requires_cooldown,
    previous_response_not_found, previous_response_not_found_value,
    previous_response_requires_websocket, prompt_cache_write_rejected,
    recoverable_response_affinity_miss, recoverable_response_model_switch,
    responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_call_output_has_invalid_call_id,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    responses_tool_call_is_missing_output, responses_tool_call_is_missing_output_message,
    responses_tool_call_links_rejected, responses_tool_call_links_rejected_value,
    retryable_failure, retryable_status, zenith_gateway_invalid_request,
    zenith_gateway_invalid_request_value,
};

#[cfg(test)]
use failure::responses_call_id_is_missing;

pub(super) use response::{
    api_error, api_error_code, api_error_type, api_error_with_origin,
    api_error_with_origin_and_category, api_error_with_parameter, cooldown_error,
    prefix_error_body, prefix_error_value,
};

pub(super) const TRANSIENT_COOLDOWN_MS: u64 = 60_000;

/// Waiting is only useful for an unavailable route, never for a request or
/// credential that must be changed before another generation is possible.
pub(super) fn retryable_recovery_wait(
    status: StatusCode,
    category: &'static str,
    has_previous_response_id: bool,
) -> bool {
    retryable_failure(status, category, has_previous_response_id)
        && !matches!(
            category,
            error_codes::UPSTREAM_UNAUTHORIZED
                | error_codes::UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED
                | error_codes::UPSTREAM_ACCOUNT_DISABLED
                | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
                | error_codes::UPSTREAM_REGION_UNSUPPORTED
                | error_codes::UPSTREAM_MODEL_NOT_FOUND
                | error_codes::UPSTREAM_MODEL_UNSUPPORTED
                | error_codes::UPSTREAM_FORBIDDEN
                | error_codes::UPSTREAM_CONTENT_POLICY
                | error_codes::UPSTREAM_INVALID_REQUEST
                | error_codes::UPSTREAM_CANDIDATE_REJECTED
        )
}

pub(super) fn admission_failure(
    reason: crate::scheduler::rotation::AdmissionStopReason,
) -> (&'static str, &'static str) {
    use crate::scheduler::rotation::AdmissionStopReason;
    match reason {
        AdmissionStopReason::QueueFull => (
            error_codes::ADMISSION_QUEUE_FULL,
            "Relay admission queue is full",
        ),
        AdmissionStopReason::WaitExpired => (
            error_codes::ADMISSION_WAIT_EXPIRED,
            "Relay request admission wait expired",
        ),
    }
}

pub(super) fn admission_error(
    budget: &crate::scheduler::rotation::SharedRequestBudget,
) -> Option<Response<Body>> {
    let (code, message) = admission_failure(budget.admission_stop_reason()?);
    Some(api_error(StatusCode::SERVICE_UNAVAILABLE, message, code))
}

const MAX_RATE_LIMIT_COOLDOWN_MS: u64 = 30 * 60_000;

const MAX_RATE_LIMIT_RETRY_HINT_MS: u64 = 7 * 24 * 60 * 60_000;

/// Marks an error body constructed by Relay itself. Native protocol handlers
/// use this marker to normalize only local errors without rewriting an
/// upstream provider's already-native error envelope.
#[derive(Clone, Copy, Debug)]
pub(super) struct LocalGatewayError;

#[derive(Clone, Copy)]
pub(super) struct AttemptFailure {
    pub(super) execution: crate::scheduler::rotation::ExecutionObservation,
    pub(super) status: StatusCode,
    pub(super) category: &'static str,
    pub(super) message: &'static str,
    pub(super) cooldown_hint: RateLimitBodyHint,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct PreservedUpstreamError {
    pub(super) status: StatusCode,
    pub(super) category: &'static str,
    pub(super) code: String,
    pub(super) message: String,
    pub(super) error_type: Option<String>,
}

/// Preserves only the bounded, redacted error envelope across retries and bridges.
pub(super) fn preserved_upstream_error(
    failure: &AttemptFailure,
    response_body: &[u8],
) -> Option<PreservedUpstreamError> {
    preserved_error_details(
        failure,
        crate::usage::UpstreamErrorDetails::from_response_body(None, response_body),
    )
}

pub(super) fn preserved_upstream_error_value(
    failure: &AttemptFailure,
    error_payload: &Value,
) -> Option<PreservedUpstreamError> {
    preserved_error_details(
        failure,
        crate::usage::UpstreamErrorDetails::from_value(None, error_payload),
    )
}

fn preserved_error_details(
    failure: &AttemptFailure,
    details: crate::usage::UpstreamErrorDetails,
) -> Option<PreservedUpstreamError> {
    Some(PreservedUpstreamError {
        status: failure.status,
        category: failure.category,
        code: details
            .code
            .unwrap_or_else(|| api_error_code(failure.category).to_string()),
        message: details.message?,
        error_type: details.error_type,
    })
}

#[derive(Clone, Debug)]
pub(super) struct FailureState {
    pub(super) cooldown_scope: Option<String>,
    pub(super) retry_at_ms: Option<u64>,
    pub(super) consecutive_failures: u32,
}

pub(super) fn upstream_failure_message(category: &str) -> &'static str {
    error_codes::upstream_message(category)
}

pub(super) fn apply_degraded_route_policy(runtime: &GatewayRuntime, failure: &mut AttemptFailure) {
    let category = runtime.effective_upstream_category(failure.category);
    if category == failure.category {
        return;
    }
    failure.category = category;
    failure.status = canonical_upstream_status(failure.status, category);
    failure.message = upstream_failure_message(category);
}

pub(super) fn upstream_failure_status(category: &str) -> StatusCode {
    StatusCode::from_u16(error_codes::upstream_status(category)).unwrap_or(StatusCode::BAD_GATEWAY)
}

pub(super) fn canonical_upstream_status(status: StatusCode, category: &str) -> StatusCode {
    if category == error_codes::UPSTREAM_STATUS
        || status == StatusCode::FORBIDDEN
            && matches!(
                category,
                error_codes::UPSTREAM_CONTENT_POLICY | error_codes::UPSTREAM_MODEL_UNAVAILABLE
            )
    {
        status
    } else {
        upstream_failure_status(category)
    }
}
pub(super) fn upstream_status_from_value(error_payload: &Value) -> Option<StatusCode> {
    [
        "/status",
        "/status_code",
        "/error/status",
        "/error/status_code",
        "/body/status",
        "/body/status_code",
        "/body/error/status",
        "/body/error/status_code",
        "/response/status",
        "/response/status_code",
        "/response/error/status",
        "/response/error/status_code",
    ]
    .into_iter()
    .filter_map(|path| error_payload.pointer(path))
    .find_map(|status_value| {
        status_value
            .as_u64()
            .or_else(|| {
                status_value
                    .as_str()
                    .and_then(|status| status.trim().parse().ok())
            })
            .and_then(|status| u16::try_from(status).ok())
            .filter(|status| *status > 0)
            .and_then(|status| StatusCode::from_u16(status).ok())
    })
}

pub(super) fn upstream_event_failure_category(
    event_type: Option<&str>,
    event_payload: &Value,
) -> Option<&'static str> {
    let event_type = if ["/error", "/response/error", "/body/error"]
        .iter()
        .any(|path| {
            event_payload
                .pointer(path)
                .is_some_and(|error| !error.is_null())
        }) {
        Some("error")
    } else {
        event_type
    };
    match event_type {
        Some("response.completed" | "response.done") => {
            match event_payload
                .pointer("/response/status")
                .and_then(Value::as_str)
            {
                Some("failed" | "cancelled" | "canceled") => Some(error_codes::UPSTREAM_TERMINAL),
                Some("incomplete") => Some(error_codes::RESPONSE_INCOMPLETE),
                Some("completed") | None => None,
                Some(_) => Some(error_codes::STREAM_INVALID),
            }
        }
        Some("response.incomplete") => Some(error_codes::RESPONSE_INCOMPLETE),
        Some("response.cancelled" | "response.canceled") => Some(error_codes::UPSTREAM_CANCELLED),
        Some("response.failed" | "error") => {
            let classification = classify_upstream_error_value(
                upstream_status_from_value(event_payload).unwrap_or(StatusCode::BAD_GATEWAY),
                event_payload,
            );
            Some(
                if classification.category == error_codes::UPSTREAM_BAD_GATEWAY
                    && upstream_status_from_value(event_payload).is_none()
                {
                    error_codes::UPSTREAM_TERMINAL
                } else {
                    classification.category
                },
            )
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests;
