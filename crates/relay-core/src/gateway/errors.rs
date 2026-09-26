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
use std::time::{SystemTime, UNIX_EPOCH};

mod cooldown;
mod failure;
mod response;

pub(super) use cooldown::{
    apply_failure_state, current_failure_state, failure_cooldown, rate_limit_body_hint,
    rate_limit_body_hint_value, settle_attempt_failure, settle_classified_failure,
    settle_image_capability_failure, settle_status_failure, RateLimitBodyHint,
};

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
    body: &[u8],
) -> Option<PreservedUpstreamError> {
    preserved_error_details(
        failure,
        crate::usage::UpstreamErrorDetails::from_body(None, body),
    )
}

pub(super) fn preserved_upstream_error_value(
    failure: &AttemptFailure,
    value: &Value,
) -> Option<PreservedUpstreamError> {
    preserved_error_details(
        failure,
        crate::usage::UpstreamErrorDetails::from_value(None, value),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct UpstreamErrorClassification {
    pub(super) category: &'static str,
    pub(super) message: &'static str,
}

pub(super) fn classify_upstream_error(
    status: StatusCode,
    body: Option<&[u8]>,
) -> UpstreamErrorClassification {
    let Some(body) = body else {
        return classify_upstream_error_text(status, "");
    };
    match serde_json::from_slice::<Value>(body) {
        Ok(value) => classify_upstream_error_value(status, &value),
        Err(_) => classify_upstream_error_text(status, &normalized_error_text(body)),
    }
}

pub(super) fn classify_upstream_error_value(
    status: StatusCode,
    value: &Value,
) -> UpstreamErrorClassification {
    if zenith_gateway_invalid_request_value(value) {
        return UpstreamErrorClassification {
            // This gateway envelope hides the actual cause, including route
            // and model access failures. It does not prove invalid client input.
            category: error_codes::UPSTREAM_CANDIDATE_REJECTED,
            message: upstream_failure_message(error_codes::UPSTREAM_CANDIDATE_REJECTED),
        };
    }
    classify_upstream_error_text(status, &upstream_error_text(value))
}

pub(crate) fn is_deactivated_workspace_value(value: &Value) -> bool {
    [
        "/detail/code",
        "/error/code",
        "/body/error/code",
        "/response/error/code",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    .any(|code| code.eq_ignore_ascii_case("deactivated_workspace"))
}

pub(crate) fn is_deactivated_workspace(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .is_some_and(|value| is_deactivated_workspace_value(&value))
}

fn classify_upstream_error_text(status: StatusCode, text: &str) -> UpstreamErrorClassification {
    let category = if text_has_any(
        text,
        &[
            error_codes::RESPONSE_CONTINUATION_UNAVAILABLE,
            "responses continuation route is unknown",
            "responses continuation is bound to a provider slot",
            "responses continuation requires the same native provider endpoint",
        ],
    ) {
        error_codes::RESPONSE_AFFINITY_MISS
    } else if text_has_any(text, &[error_codes::REFRESH_TOKEN_REUSED]) {
        // A refresh-token race is transient. The token authority retries it
        // without changing account authentication state, and a proxied
        // upstream response must follow the same rule.
        error_codes::UPSTREAM_REFRESH_TOKEN_REUSED
    } else if text_has_any(
        text,
        &[
            "previous_response_not_found",
            "invalid_previous_response_id",
            "previous response not found",
            "no response found for previous_response_id",
            "unknown or expired previous_response_id",
        ],
    ) {
        error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
    } else if text_has_any(
        text,
        &[
            "phone_verification_required",
            "phone number verification required",
            "verify your phone number",
            "account_verification_required",
            "account verification required",
            "verify your account",
            "account must be verified",
        ],
    ) {
        error_codes::UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED
    } else if text_has_any(
        text,
        &[
            "tool_call_not_found",
            "no tool call found for",
            "no matching tool call",
            "tool call output does not match",
            "unanswered_function_call",
            "no tool output found for function call",
            "no tool output found for custom tool call",
            "no tool output found for apply patch call",
        ],
    ) || failure::responses_call_id_is_missing_text(text)
    {
        error_codes::UPSTREAM_TOOL_CALL_MISMATCH
    } else if text_has_any(
        text,
        &[
            "context_length_exceeded",
            "context_window_exceeded",
            "context_too_large",
            "maximum context length",
            "max context length",
        ],
    ) || (text.contains("context window")
        && text_has_any(text, &["exceed", "too large", "too long"]))
        || (text.contains("context length")
            && text_has_any(text, &["exceed", "too large", "too long"]))
    {
        error_codes::UPSTREAM_CONTEXT_TOO_LARGE
    } else if text_has_any(
        text,
        &[
            "invalid_encrypted_content",
            "thinking_signature_invalid",
            "invalid signature in thinking block",
            "encrypted content could not be verified",
        ],
    ) {
        error_codes::UPSTREAM_ENCRYPTED_CONTENT_INVALID
    } else if text_has_any(
        text,
        &[
            "instructions are required",
            "required parameter: 'instructions'",
            "required parameter: instructions",
        ],
    ) {
        error_codes::UPSTREAM_INSTRUCTIONS_REQUIRED
    } else if text_has_any(
        text,
        &[
            "account_deactivated",
            "account_disabled",
            "account_expired",
            "organization_deactivated",
            "organization_disabled",
            "project_deactivated",
            "deactivated_workspace",
            "workspace_disabled",
            "workspace_expired",
            "workspace_terminated",
            "account has been deactivated",
            "account is disabled",
        ],
    ) {
        error_codes::UPSTREAM_ACCOUNT_DISABLED
    } else if text_has_any(
        text,
        &[
            "usage_not_included",
            "not included in your plan",
            "subscription does not include",
        ],
    ) {
        error_codes::UPSTREAM_USAGE_NOT_INCLUDED
    } else if text_has_any(
        text,
        &[
            "insufficient_quota",
            "usage_limit_reached",
            "usage_limit_exceeded",
            "usage limit reached",
            error_codes::QUOTA_EXHAUSTED,
            "quota exceeded",
            "billing_hard_limit_reached",
            "organization_spend_limit_exceeded",
            "project_spend_limit_exceeded",
            "organization_usage_limit_exceeded",
            "credit_balance_exhausted",
            "credits_exhausted",
            "credits exhausted",
            "exceeded your current quota",
            "out of credits",
            "add credits to continue",
        ],
    ) || status == StatusCode::PAYMENT_REQUIRED
    {
        error_codes::UPSTREAM_QUOTA_EXHAUSTED
    } else if text_has_any(
        text,
        &[
            error_codes::INVALID_API_KEY,
            "authentication_error",
            "invalid authentication",
            "invalid bearer token",
            "expired_token",
            "token_expired",
            error_codes::TOKEN_INVALIDATED,
            "token_revoked",
            "invalid or expired token",
            error_codes::INVALID_GRANT,
        ],
    ) || status == StatusCode::UNAUTHORIZED
    {
        error_codes::UPSTREAM_UNAUTHORIZED
    } else if text_has_any(
        text,
        &[
            "unsupported_country_region_territory",
            "country_not_supported",
            "region_not_supported",
            "country, region, or territory not supported",
        ],
    ) {
        error_codes::UPSTREAM_REGION_UNSUPPORTED
    } else if text_has_any(
        text,
        &[
            "content_policy_violation",
            "content_filter",
            "policy_violation",
            "safety_violation",
            "cyber_policy",
            "bio_policy",
            "content_moderation_failed",
        ],
    ) {
        error_codes::UPSTREAM_CONTENT_POLICY
    } else if text_has_any(
        text,
        &[
            "model_disabled",
            "requested model is disabled",
            "this route cannot serve the request",
        ],
    ) {
        // An explicit route rejection is scoped to that model, not a generic
        // invalid input and not a vote against the provider's inference health.
        error_codes::UPSTREAM_CANDIDATE_REJECTED
    } else if text_has_any(
        text,
        &[
            "invalid_prompt",
            "invalid_argument",
            "validation_error",
            "invalid call_id for function_call_output",
            "invalid call id for function_call_output",
            "invalid_call_id_for_function_call_output",
            "invalid_function_call_output_call_id",
        ],
    ) || (text.contains("input[")
        && text.contains(".id")
        && text_has_any(
            text,
            &[
                "expected an id that begins with 'fc'",
                "expected an id that begins with 'ctc'",
                "expected an id that begins with 'ctc_'",
                "expected an id that starts with 'ctc'",
                "expected an id that begins with 'msg'",
            ],
        ))
    {
        error_codes::UPSTREAM_INVALID_REQUEST
    } else if status == StatusCode::PAYLOAD_TOO_LARGE
        || text_has_any(
            text,
            &[
                error_codes::REQUEST_TOO_LARGE,
                "payload_too_large",
                "content_too_large",
                "request body too large",
                "length limit exceeded",
            ],
        )
    {
        error_codes::UPSTREAM_PAYLOAD_TOO_LARGE
    } else if text_has_any(
        text,
        &[
            "unsupported_parameter",
            error_codes::UNSUPPORTED_VALUE,
            "invalid_parameter",
            "parameter_not_supported",
        ],
    ) {
        error_codes::UPSTREAM_UNSUPPORTED_REQUEST
    } else if text_has_any(
        text,
        &[
            "model_at_capacity",
            "selected model is at capacity",
            "model is at capacity",
        ],
    ) {
        error_codes::UPSTREAM_MODEL_CAPACITY
    } else if text.contains("model_not_available") {
        error_codes::UPSTREAM_MODEL_UNAVAILABLE
    } else if text.contains(error_codes::MODEL_NOT_FOUND) {
        error_codes::UPSTREAM_MODEL_NOT_FOUND
    } else if status == StatusCode::NOT_ACCEPTABLE
        || text_has_any(
            text,
            &[
                "model_not_supported",
                "requested model is not supported",
                "model is not supported when using codex with a chatgpt account",
                "is not currently available for this chatgpt account",
            ],
        )
        || (text.contains("model")
            && text.contains("does not exist or you do not have access to it"))
    {
        error_codes::UPSTREAM_MODEL_UNSUPPORTED
    } else if status == StatusCode::UPGRADE_REQUIRED
        || text_has_any(text, &["websocket_not_supported", "websocket_unsupported"])
    {
        error_codes::UPSTREAM_WEBSOCKET_UNSUPPORTED
    } else if text.contains("websocket_connection_limit_reached") {
        error_codes::UPSTREAM_WEBSOCKET_CONNECTION_LIMIT
    } else if text_has_any(
        text,
        &[
            "rate_limit_exceeded",
            "rate_limit_error",
            "rate_limit_reached",
            "rate limit reached",
            "rate limit exceeded",
            "too many requests",
            "slow_down",
            "slow down",
        ],
    ) {
        error_codes::UPSTREAM_RATE_LIMITED
    } else if status.as_u16() == 529
        || text_has_any(
            text,
            &["server_is_overloaded", "server_overloaded", "overloaded"],
        )
    {
        error_codes::UPSTREAM_OVERLOADED
    } else if text_has_any(text, &["service_unavailable", "temporarily unavailable"]) {
        error_codes::UPSTREAM_UNAVAILABLE
    } else if text_has_any(
        text,
        &[
            "internal_server_error",
            "server_error",
            "an error occurred while processing your request",
        ],
    ) || (text.contains("you can retry your request") && text.contains("request id"))
    {
        error_codes::UPSTREAM_SERVER_ERROR
    } else if status == StatusCode::FORBIDDEN
        && text_has_any(
            text,
            &[
                "cf-mitigated",
                "cf-chl-bypass",
                "_cf_chl",
                "cf_chl",
                "attention required",
                "just a moment",
            ],
        )
    {
        error_codes::UPSTREAM_EDGE_CHALLENGE
    } else if status == StatusCode::FORBIDDEN {
        error_codes::UPSTREAM_FORBIDDEN
    } else if status == StatusCode::NOT_FOUND {
        error_codes::UPSTREAM_NOT_FOUND
    } else if status == StatusCode::REQUEST_TIMEOUT {
        error_codes::UPSTREAM_REQUEST_TIMEOUT
    } else if status == StatusCode::CONFLICT {
        error_codes::UPSTREAM_CONFLICT
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        error_codes::UPSTREAM_RATE_LIMITED
    } else if status == StatusCode::INTERNAL_SERVER_ERROR {
        error_codes::UPSTREAM_SERVER_ERROR
    } else if status == StatusCode::BAD_GATEWAY {
        error_codes::UPSTREAM_BAD_GATEWAY
    } else if status == StatusCode::SERVICE_UNAVAILABLE {
        error_codes::UPSTREAM_UNAVAILABLE
    } else if status == StatusCode::GATEWAY_TIMEOUT {
        error_codes::UPSTREAM_GATEWAY_TIMEOUT
    } else if status.is_client_error() {
        error_codes::UPSTREAM_INVALID_REQUEST
    } else if status.is_server_error() {
        error_codes::UPSTREAM_SERVER_ERROR
    } else {
        error_codes::UPSTREAM_STATUS
    };
    UpstreamErrorClassification {
        category,
        message: upstream_failure_message(category),
    }
}

pub(super) fn upstream_failure_message(category: &str) -> &'static str {
    error_codes::upstream_message(category)
}

pub(super) fn upstream_failure_status(category: &str) -> StatusCode {
    StatusCode::from_u16(error_codes::upstream_status(category)).unwrap_or(StatusCode::BAD_GATEWAY)
}

pub(super) fn canonical_upstream_status(status: StatusCode, category: &str) -> StatusCode {
    if category == error_codes::UPSTREAM_STATUS {
        status
    } else {
        upstream_failure_status(category)
    }
}

fn upstream_error_text(value: &Value) -> String {
    const PATHS: &[&str] = &[
        "/code",
        "/type",
        "/message",
        "/msg",
        "/err",
        "/error_msg",
        "/detail",
        "/error_code",
        "/error",
        "/error/code",
        "/error/type",
        "/error/message",
        "/error/detail",
        "/error/status",
        "/detail/code",
        "/detail/type",
        "/detail/message",
        "/body/code",
        "/body/type",
        "/body/message",
        "/body/error",
        "/body/error/code",
        "/body/error/type",
        "/body/error/message",
        "/response/code",
        "/response/type",
        "/response/message",
        "/response/error",
        "/response/error/code",
        "/response/error/type",
        "/response/error/message",
        "/response/incomplete_details/reason",
        "/header/message",
    ];
    let mut text = String::new();
    for value in PATHS
        .iter()
        .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    {
        if !text.is_empty() {
            text.push(' ');
        }
        text.extend(
            value
                .chars()
                .take(4_096)
                .map(|character| character.to_ascii_lowercase()),
        );
    }
    text
}

fn normalized_error_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .chars()
        .take(4_096)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

fn text_has_any(text: &str, values: &[&str]) -> bool {
    values.iter().any(|value| text.contains(value))
}

pub(super) fn upstream_status_from_value(value: &Value) -> Option<StatusCode> {
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
    .filter_map(|path| value.pointer(path))
    .find_map(|value| {
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|status| status.trim().parse().ok()))
            .and_then(|status| u16::try_from(status).ok())
            .filter(|status| *status > 0)
            .and_then(|status| StatusCode::from_u16(status).ok())
    })
}

pub(super) fn upstream_event_failure_category(
    event_type: Option<&str>,
    value: &Value,
) -> Option<&'static str> {
    let event_type = if ["/error", "/response/error", "/body/error"]
        .iter()
        .any(|path| value.pointer(path).is_some_and(|error| !error.is_null()))
    {
        Some("error")
    } else {
        event_type
    };
    match event_type {
        Some("response.completed" | "response.done") => {
            match value.pointer("/response/status").and_then(Value::as_str) {
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
                upstream_status_from_value(value).unwrap_or(StatusCode::BAD_GATEWAY),
                value,
            );
            Some(
                if classification.category == error_codes::UPSTREAM_BAD_GATEWAY
                    && upstream_status_from_value(value).is_none()
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
