use super::now_ms;
use crate::runtime::{AuthorizedRequestError, ExecutorPrepareError};
use crate::scheduler::{CandidateScope, CooldownReason, CooldownRequest};
use crate::{GatewayRuntime, UsageEvent, WireApi};
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
    apply_attempt_failure_cooldown, apply_cooldown, apply_cooldown_for_model,
    apply_failure_cooldown_with_body, apply_failure_cooldown_with_hint, apply_failure_state,
    apply_mandatory_cooldown, rate_limit_body_hint, rate_limit_body_hint_value, RateLimitBodyHint,
};

pub(super) use failure::{
    failure_category_is_request_terminal, failure_category_requires_cooldown,
    previous_response_not_found, previous_response_not_found_value,
    previous_response_requires_websocket, recoverable_response_affinity_miss,
    responses_custom_tool_item_id_requires_ctc_prefix,
    responses_function_call_output_has_invalid_call_id,
    responses_function_item_id_requires_fc_prefix, responses_message_item_id_requires_msg_prefix,
    retry_candidate_limit, retryable_failure, retryable_status, zenith_gateway_invalid_request,
    zenith_gateway_invalid_request_value,
};

pub(super) use response::{
    api_error, api_error_code, api_error_type, api_error_with_origin,
    api_error_with_origin_and_category, cooldown_error,
};

pub(super) const TRANSIENT_COOLDOWN_MS: u64 = 60_000;

const MAX_RESPONSE_OWNER_CANDIDATES: usize = 8;

const MAX_SAFE_UPSTREAM_ERROR_MESSAGE_CHARS: usize = 1_024;

const MAX_RATE_LIMIT_COOLDOWN_MS: u64 = 30 * 60_000;

const MAX_RATE_LIMIT_RETRY_HINT_MS: u64 = 7 * 24 * 60 * 60_000;

pub(super) struct CooldownContext<'a> {
    pub(super) scope: &'a CandidateScope,
    pub(super) allowed_protocols: &'a [WireApi],
}

/// Marks an error body constructed by Relay itself. Native protocol handlers
/// use this marker to normalize only local errors without rewriting an
/// upstream provider's already-native error envelope.
#[derive(Clone, Copy, Debug)]
pub(super) struct LocalGatewayError;

#[derive(Clone, Copy)]
pub(super) struct AttemptFailure {
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
}

/// Extracts only a short, structured upstream error message for the final
/// response after retries are exhausted. Raw upstream bodies are intentionally
/// never forwarded because they may contain URLs, credentials, or diagnostics.
pub(super) fn preserved_upstream_error(
    failure: &AttemptFailure,
    body: &[u8],
) -> Option<PreservedUpstreamError> {
    let value = serde_json::from_slice::<Value>(body).ok()?;
    preserved_upstream_error_value(failure, &value)
}

pub(super) fn preserved_upstream_error_value(
    failure: &AttemptFailure,
    value: &Value,
) -> Option<PreservedUpstreamError> {
    let code = [
        "/error/code",
        "/response/error/code",
        "/body/error/code",
        "/code",
        "/response/code",
        "/body/code",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .find(|code| !code.is_empty() && code.len() <= 128 && !code.chars().any(char::is_control))?;
    if !is_public_gateway_error_code(code) {
        return None;
    }
    let message = [
        "/error/message",
        "/response/error/message",
        "/body/error/message",
        "/message",
        "/response/message",
        "/body/message",
        "/error/detail",
        "/response/error/detail",
        "/body/error/detail",
        "/detail",
    ]
    .into_iter()
    .filter_map(|path| value.pointer(path).and_then(Value::as_str))
    .map(str::trim)
    .find(|message| {
        !message.is_empty()
            && message.chars().count() <= MAX_SAFE_UPSTREAM_ERROR_MESSAGE_CHARS
            && !message.chars().any(char::is_control)
            && !contains_sensitive_error_metadata(message)
    })?;
    Some(PreservedUpstreamError {
        status: failure.status,
        category: failure.category,
        code: code.to_string(),
        message: message.to_string(),
    })
}

fn is_public_gateway_error_code(code: &str) -> bool {
    matches!(
        code.trim(),
        "service_unavailable"
            | "bad_request"
            | "invalid_request"
            | "invalid_prompt"
            | "context_length_exceeded"
            | "request_too_large"
            | "content_policy_violation"
            | "model_not_available"
            | "model_not_found"
            | "model_disabled"
            | "invalid_image_size"
            | "unauthorized"
            | "forbidden"
            | "insufficient_balance"
            | "rate_limit_exceeded"
            | "not_found"
            | "service_timeout"
            | "internal_error"
            | "server_error"
            | "no_eligible_source"
            | "all_sources_temporarily_unavailable"
            | "all_sources_cooling_down"
            | "server_is_overloaded"
            | "bad_gateway"
            | "gateway_timeout"
    )
}

fn contains_sensitive_error_metadata(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "http://",
        "https://",
        "url:",
        "authorization",
        "bearer ",
        "api_key",
        "apikey",
        "secret",
        "cookie",
        "token:",
        "sk-",
        "@",
        "org-",
        "user-",
    ]
    .into_iter()
    .any(|marker| normalized.contains(marker))
}

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
            "response_continuation_unavailable",
            "responses continuation route is unknown",
            "responses continuation is bound to a provider slot",
            "responses continuation requires the same native provider endpoint",
        ],
    ) {
        "response_affinity_miss"
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
        "upstream_previous_response_not_found"
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
        "upstream_account_verification_required"
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
    ) {
        "upstream_tool_call_mismatch"
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
        "upstream_context_too_large"
    } else if text_has_any(
        text,
        &[
            "invalid_encrypted_content",
            "thinking_signature_invalid",
            "invalid signature in thinking block",
            "encrypted content could not be verified",
        ],
    ) {
        "upstream_encrypted_content_invalid"
    } else if text_has_any(
        text,
        &[
            "instructions are required",
            "required parameter: 'instructions'",
            "required parameter: instructions",
        ],
    ) {
        "upstream_instructions_required"
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
        "upstream_account_disabled"
    } else if text_has_any(
        text,
        &[
            "usage_not_included",
            "not included in your plan",
            "subscription does not include",
        ],
    ) {
        "upstream_usage_not_included"
    } else if text_has_any(
        text,
        &[
            "insufficient_quota",
            "usage_limit_reached",
            "usage_limit_exceeded",
            "usage limit reached",
            "quota_exhausted",
            "quota exceeded",
            "billing_hard_limit_reached",
            "credit_balance_exhausted",
            "credits_exhausted",
            "credits exhausted",
            "exceeded your current quota",
            "out of credits",
            "add credits to continue",
        ],
    ) || status == StatusCode::PAYMENT_REQUIRED
    {
        "upstream_quota_exhausted"
    } else if text_has_any(
        text,
        &[
            "invalid_api_key",
            "authentication_error",
            "invalid authentication",
            "invalid bearer token",
            "expired_token",
            "token_expired",
            "token_invalidated",
            "token_revoked",
            "refresh_token_reused",
            "invalid or expired token",
            "invalid_grant",
        ],
    ) || status == StatusCode::UNAUTHORIZED
    {
        "upstream_unauthorized"
    } else if text_has_any(
        text,
        &[
            "unsupported_country_region_territory",
            "country_not_supported",
            "region_not_supported",
            "country, region, or territory not supported",
        ],
    ) {
        "upstream_region_unsupported"
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
        "upstream_content_policy"
    } else if text_has_any(text, &["invalid_prompt"]) {
        "upstream_invalid_request"
    } else if status == StatusCode::PAYLOAD_TOO_LARGE
        || text_has_any(
            text,
            &[
                "request_too_large",
                "payload_too_large",
                "content_too_large",
                "request body too large",
                "length limit exceeded",
            ],
        )
    {
        "upstream_payload_too_large"
    } else if text_has_any(
        text,
        &[
            "unsupported_parameter",
            "unsupported_value",
            "invalid_parameter",
            "parameter_not_supported",
        ],
    ) {
        "upstream_unsupported_request"
    } else if text_has_any(
        text,
        &[
            "model_at_capacity",
            "selected model is at capacity",
            "model is at capacity",
        ],
    ) {
        "upstream_model_capacity"
    } else if text_has_any(text, &["model_not_found", "model_not_available"]) {
        "upstream_model_not_found"
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
        "upstream_model_unsupported"
    } else if status == StatusCode::UPGRADE_REQUIRED
        || text_has_any(text, &["websocket_not_supported", "websocket_unsupported"])
    {
        "upstream_websocket_unsupported"
    } else if text.contains("websocket_connection_limit_reached") {
        "upstream_websocket_connection_limit"
    } else if text_has_any(
        text,
        &[
            "rate_limit_exceeded",
            "rate_limit_error",
            "rate_limit_reached",
            "rate limit reached",
            "rate limit exceeded",
            "too many requests",
        ],
    ) {
        "upstream_rate_limited"
    } else if status.as_u16() == 529
        || text_has_any(
            text,
            &[
                "server_is_overloaded",
                "server_overloaded",
                "overloaded",
                "slow_down",
                "slow down",
            ],
        )
    {
        "upstream_overloaded"
    } else if text_has_any(text, &["service_unavailable", "temporarily unavailable"]) {
        "upstream_unavailable"
    } else if text_has_any(
        text,
        &[
            "internal_server_error",
            "server_error",
            "an error occurred while processing your request",
        ],
    ) || (text.contains("you can retry your request") && text.contains("request id"))
    {
        "upstream_server_error"
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
        "upstream_edge_challenge"
    } else if text_has_any(
        text,
        &[
            "invalid_request",
            "invalid request",
            "request is invalid",
            "bad_request",
            "bad request",
        ],
    ) {
        "upstream_invalid_request"
    } else if status == StatusCode::FORBIDDEN {
        "upstream_forbidden"
    } else if status == StatusCode::NOT_FOUND {
        "upstream_not_found"
    } else if status == StatusCode::REQUEST_TIMEOUT {
        "upstream_request_timeout"
    } else if status == StatusCode::CONFLICT {
        "upstream_conflict"
    } else if status == StatusCode::UNPROCESSABLE_ENTITY {
        "upstream_invalid_request"
    } else if status == StatusCode::TOO_MANY_REQUESTS {
        "upstream_rate_limited"
    } else if status == StatusCode::INTERNAL_SERVER_ERROR {
        "upstream_server_error"
    } else if status == StatusCode::BAD_GATEWAY {
        "upstream_bad_gateway"
    } else if status == StatusCode::SERVICE_UNAVAILABLE {
        "upstream_unavailable"
    } else if status == StatusCode::GATEWAY_TIMEOUT {
        "upstream_gateway_timeout"
    } else if status == StatusCode::BAD_REQUEST {
        "upstream_invalid_request"
    } else if status.is_server_error() {
        "upstream_server_error"
    } else {
        "upstream_status"
    };
    UpstreamErrorClassification {
        category,
        message: upstream_failure_message(category),
    }
}

pub(super) fn upstream_failure_message(category: &str) -> &'static str {
    match category {
        "response_affinity_miss" => "Responses continuation route is unavailable",
        "upstream_previous_response_not_found" => "previous response is unavailable",
        "upstream_tool_call_mismatch" => "tool output does not match an active tool call",
        "upstream_context_too_large" => "request context exceeds the model limit",
        "upstream_encrypted_content_invalid" => "encrypted reasoning context is invalid",
        "upstream_instructions_required" => "upstream requires request instructions",
        "upstream_usage_not_included" => "upstream account plan does not include this capability",
        "upstream_quota_exhausted" => "upstream usage quota is exhausted",
        "upstream_account_verification_required" => "upstream account verification is required",
        "upstream_account_disabled" => "upstream account is disabled",
        "upstream_unauthorized" => "upstream authentication failed",
        "upstream_region_unsupported" => "upstream rejected the request region",
        "upstream_content_policy" => "upstream content policy rejected the request",
        "upstream_payload_too_large" => "upstream rejected the request size",
        "upstream_unsupported_request" => "upstream does not support this request",
        "upstream_model_not_found" => "upstream model is unavailable",
        "upstream_model_unsupported" => "upstream does not support this model",
        "upstream_model_capacity" => "upstream model is at capacity",
        "upstream_websocket_unsupported" => "upstream does not support WebSocket requests",
        "upstream_websocket_connection_limit" => "upstream WebSocket connection limit was reached",
        "upstream_rate_limited" => "upstream rate limit was reached",
        "upstream_edge_challenge" => "upstream edge security challenged the request",
        "upstream_forbidden" => "upstream access was forbidden",
        "upstream_not_found" => "upstream resource was not found",
        "upstream_request_timeout" => "upstream request timed out",
        "upstream_conflict" => "upstream request conflicted with current state",
        "upstream_invalid_request" => "upstream rejected the request",
        "upstream_overloaded" => "upstream service is overloaded",
        "upstream_server_error" => "upstream service failed",
        "upstream_bad_gateway" => "upstream gateway failed",
        "upstream_unavailable" => "upstream service is unavailable",
        "upstream_gateway_timeout" => "upstream gateway timed out",
        _ => "all eligible upstream sources failed",
    }
}

pub(super) fn upstream_failure_status(category: &str) -> StatusCode {
    match category {
        "upstream_unauthorized" => StatusCode::UNAUTHORIZED,
        "upstream_account_disabled"
        | "upstream_account_verification_required"
        | "upstream_forbidden"
        | "upstream_region_unsupported" => StatusCode::FORBIDDEN,
        "upstream_usage_not_included" => StatusCode::FORBIDDEN,
        "upstream_quota_exhausted"
        | "upstream_rate_limited"
        | "upstream_websocket_connection_limit" => StatusCode::TOO_MANY_REQUESTS,
        "upstream_model_not_found" | "upstream_not_found" => StatusCode::NOT_FOUND,
        "upstream_model_unsupported" => StatusCode::NOT_ACCEPTABLE,
        "upstream_websocket_unsupported" => StatusCode::UPGRADE_REQUIRED,
        "upstream_request_timeout" => StatusCode::REQUEST_TIMEOUT,
        "upstream_conflict" => StatusCode::CONFLICT,
        "upstream_payload_too_large" => StatusCode::PAYLOAD_TOO_LARGE,
        "response_affinity_miss"
        | "upstream_previous_response_not_found"
        | "upstream_tool_call_mismatch"
        | "upstream_context_too_large"
        | "upstream_encrypted_content_invalid"
        | "upstream_instructions_required"
        | "upstream_content_policy"
        | "upstream_unsupported_request"
        | "upstream_invalid_request" => StatusCode::BAD_REQUEST,
        "upstream_model_capacity"
        | "upstream_overloaded"
        | "upstream_unavailable"
        | "upstream_edge_challenge" => StatusCode::SERVICE_UNAVAILABLE,
        "upstream_server_error" => StatusCode::INTERNAL_SERVER_ERROR,
        "upstream_gateway_timeout" => StatusCode::GATEWAY_TIMEOUT,
        _ => StatusCode::BAD_GATEWAY,
    }
}

pub(super) fn canonical_upstream_status(status: StatusCode, category: &str) -> StatusCode {
    if category == "upstream_status" {
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
    match event_type {
        Some("response.incomplete") => Some("response_incomplete"),
        Some("response.cancelled" | "response.canceled") => Some("upstream_cancelled"),
        Some("response.failed" | "error") => {
            let classification = classify_upstream_error_value(
                upstream_status_from_value(value).unwrap_or(StatusCode::BAD_GATEWAY),
                value,
            );
            Some(
                if classification.category == "upstream_bad_gateway"
                    && upstream_status_from_value(value).is_none()
                {
                    "upstream_terminal"
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
