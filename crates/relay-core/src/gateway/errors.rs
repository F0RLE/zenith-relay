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

fn classify_upstream_error_text(status: StatusCode, text: &str) -> UpstreamErrorClassification {
    let category = if text_has_any(
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
        "upstream_previous_response_not_found"
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

impl AttemptFailure {
    pub(super) fn authorized_request(error: AuthorizedRequestError) -> Self {
        match error {
            AuthorizedRequestError::Prepare(error) => Self::prepare(error),
            AuthorizedRequestError::Transport(error) => Self::transport(&error),
            AuthorizedRequestError::NotReplayable => Self::body(),
        }
    }

    pub(super) fn transport(error: &reqwest::Error) -> Self {
        let (category, message) = if error.is_timeout() {
            ("upstream_transport_timeout", "upstream request timed out")
        } else if error.is_connect() {
            (
                "upstream_transport_connect",
                "upstream connection could not be established",
            )
        } else if error.is_body() {
            (
                "upstream_transport_body",
                "upstream request or response body failed",
            )
        } else if error.is_request() {
            ("upstream_transport_request", "upstream request failed")
        } else {
            ("upstream_transport", "upstream transport failed")
        };
        Self {
            status: StatusCode::BAD_GATEWAY,
            category,
            message,
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(super) fn body() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category: "upstream_error",
            message: "upstream response failed",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(super) fn invalid_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            category: "invalid_request",
            message: "request cannot be translated for an eligible source",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(super) fn status_with_body(status: StatusCode, body: Option<&[u8]>) -> Self {
        let classification = classify_upstream_error(status, body);
        Self {
            status: canonical_upstream_status(status, classification.category),
            category: classification.category,
            message: classification.message,
            cooldown_hint: body.map(rate_limit_body_hint).unwrap_or_default(),
        }
    }

    pub(super) fn classified_with_hint(
        status: StatusCode,
        category: &'static str,
        cooldown_hint: RateLimitBodyHint,
    ) -> Self {
        Self {
            status: canonical_upstream_status(status, category),
            category,
            message: upstream_failure_message(category),
            cooldown_hint,
        }
    }

    pub(super) fn stream(category: &'static str) -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            category,
            message: "upstream stream failed before the first event",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(super) fn no_candidate() -> Self {
        Self {
            status: StatusCode::SERVICE_UNAVAILABLE,
            category: "no_eligible_source",
            message: "no eligible source is available for this model",
            cooldown_hint: RateLimitBodyHint::default(),
        }
    }

    pub(super) fn prepare(error: ExecutorPrepareError) -> Self {
        match error {
            ExecutorPrepareError::Authentication | ExecutorPrepareError::InvalidCredential => {
                Self {
                    status: StatusCode::UNAUTHORIZED,
                    category: "account_auth",
                    message: "account authorization is unavailable",
                    cooldown_hint: RateLimitBodyHint::default(),
                }
            }
            ExecutorPrepareError::Persistence => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                category: "account_token_persistence",
                message: "refreshed account authorization could not be persisted",
                cooldown_hint: RateLimitBodyHint::default(),
            },
            ExecutorPrepareError::Transient => Self {
                status: StatusCode::BAD_GATEWAY,
                category: "account_refresh",
                message: "account authorization refresh failed",
                cooldown_hint: RateLimitBodyHint::default(),
            },
        }
    }
}

pub(super) fn retryable_status(status: StatusCode, has_previous_response_id: bool) -> bool {
    matches!(
        status,
        StatusCode::UNAUTHORIZED
            | StatusCode::PAYMENT_REQUIRED
            | StatusCode::FORBIDDEN
            | StatusCode::REQUEST_TIMEOUT
            | StatusCode::CONFLICT
            | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
        || (status == StatusCode::NOT_FOUND && !has_previous_response_id)
}

pub(super) fn retryable_failure(
    status: StatusCode,
    category: &str,
    has_previous_response_id: bool,
) -> bool {
    if !failure_category_requires_cooldown(category) {
        return false;
    }
    retryable_status(status, has_previous_response_id)
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
                | "upstream_rate_limited"
                | "upstream_request_timeout"
                | "upstream_overloaded"
                | "upstream_edge_challenge"
                | "upstream_server_error"
                | "upstream_bad_gateway"
                | "upstream_unavailable"
                | "upstream_gateway_timeout"
        )
}

/// A shared endpoint is unlikely to recover by retrying an equivalent API
/// credential in the same request. Keep that retry budget for an independent
/// endpoint and avoid cooling credentials that were never attempted.
pub(super) fn failure_requires_independent_source_endpoint(
    status: StatusCode,
    category: &str,
) -> bool {
    status.is_server_error()
        || matches!(
            category,
            "upstream_request_timeout" | "upstream_transport_timeout"
        )
}

pub(super) fn failure_category_requires_cooldown(category: &str) -> bool {
    !matches!(
        category,
        "client_cancelled"
            | "response_affinity_miss"
            | "response_incomplete"
            | "upstream_cancelled"
            | "upstream_stream"
            | "stream_incomplete"
            | "stream_idle_timeout"
            | "upstream_previous_response_not_found"
            | "upstream_tool_call_mismatch"
            | "upstream_context_too_large"
            | "upstream_encrypted_content_invalid"
            | "upstream_instructions_required"
            | "upstream_content_policy"
            | "upstream_payload_too_large"
            | "upstream_unsupported_request"
            | "upstream_websocket_unsupported"
            | "upstream_invalid_request"
    )
}

pub(super) fn failure_category_is_request_terminal(category: &str) -> bool {
    matches!(
        category,
        "upstream_tool_call_mismatch"
            | "upstream_context_too_large"
            | "upstream_encrypted_content_invalid"
            | "upstream_instructions_required"
            | "upstream_content_policy"
            | "upstream_payload_too_large"
            | "upstream_unsupported_request"
            | "upstream_websocket_unsupported"
            | "upstream_invalid_request"
    )
}

pub(super) fn recoverable_response_affinity_miss(
    status: StatusCode,
    has_previous_response_id: bool,
    _response_affinity_hit: bool,
    previous_response_not_found: bool,
) -> bool {
    has_previous_response_id
        && previous_response_not_found
        && matches!(status, StatusCode::BAD_REQUEST | StatusCode::NOT_FOUND)
}

pub(super) fn retry_candidate_limit(
    max_retry_candidates: usize,
    owner_recovery_confirmed: bool,
) -> usize {
    if owner_recovery_confirmed {
        MAX_RESPONSE_OWNER_CANDIDATES
    } else {
        max_retry_candidates
    }
}

pub(super) fn previous_response_not_found(payload: &[u8]) -> bool {
    serde_json::from_slice::<Value>(payload)
        .ok()
        .is_some_and(|value| previous_response_not_found_value(&value))
}

pub(super) fn previous_response_requires_websocket(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    let text = serde_json::to_string(&value)
        .unwrap_or_default()
        .to_ascii_lowercase();
    text.contains("previous_response_id") && text.contains("websocket")
}

pub(super) fn responses_function_call_output_has_invalid_call_id(payload: &[u8]) -> bool {
    let Ok(value) = serde_json::from_slice::<Value>(payload) else {
        return false;
    };
    let text = upstream_error_text(&value);
    text_has_any(
        &text,
        &[
            "invalid call_id for function_call_output",
            "invalid call id for function_call_output",
            "invalid_call_id_for_function_call_output",
            "invalid_function_call_output_call_id",
        ],
    )
}

/// Strict Responses endpoints use a separate `fc_` namespace for
/// `function_call.id`; the matching `call_id` is unchanged. This is only a
/// recovery signal — the request repair itself still verifies that it has a
/// call-prefixed function item before retrying.
pub(super) fn responses_function_item_id_requires_fc_prefix(payload: &[u8]) -> bool {
    let text = normalized_error_text(payload);
    text.contains("input") && text.contains("expected an id that begins with 'fc'")
}

/// Strict Responses endpoints require server-owned `msg_` item identifiers on
/// message inputs. This only identifies the precise upstream validation error;
/// the repair still verifies the foreign `item_` identifier before retrying.
pub(super) fn responses_message_item_id_requires_msg_prefix(payload: &[u8]) -> bool {
    let text = normalized_error_text(payload);
    text.contains("input[")
        && text.contains(".id")
        && text.contains("expected an id that begins with 'msg'")
}

pub(super) fn previous_response_not_found_value(value: &Value) -> bool {
    [value.pointer("/error/code"), value.pointer("/error/type")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|value| {
            value
                .trim()
                .eq_ignore_ascii_case("previous_response_not_found")
        })
        || [value.pointer("/error/message"), value.get("message")]
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(previous_response_not_found_message)
}

fn previous_response_not_found_message(message: &str) -> bool {
    let message = message.trim().trim_end_matches('.').to_ascii_lowercase();
    message == "previous response not found"
        || (message.starts_with("previous response with id ") && message.ends_with(" not found"))
        || message.starts_with("no response found for previous_response_id ")
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_failure_cooldown_with_body(
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

pub(super) fn apply_attempt_failure_cooldown(
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
pub(super) struct RateLimitBodyHint {
    pub(super) retry_after_ms: Option<u64>,
    pub(super) global: bool,
}

#[allow(clippy::too_many_arguments)]
fn apply_status_cooldown_with_hint(
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
    let now = now_system
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64;
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
            (if hint.global { "*" } else { model }, duration_ms)
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

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_failure_cooldown_with_hint(
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

fn failure_cooldown_reason(
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

pub(super) fn rate_limit_body_hint(body: &[u8]) -> RateLimitBodyHint {
    rate_limit_body_hint_at(body, SystemTime::now())
}

fn rate_limit_body_hint_at(body: &[u8], now: SystemTime) -> RateLimitBodyHint {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return RateLimitBodyHint::default();
    };
    rate_limit_body_hint_value(&value, now)
}

pub(super) fn rate_limit_body_hint_value(value: &Value, now: SystemTime) -> RateLimitBodyHint {
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

fn rate_limit_reset_delay_ms(value: &Value, now: SystemTime) -> Option<u64> {
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

fn json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|value| value.trim().parse().ok()))
}

fn json_seconds_to_ms(value: &Value) -> Option<u64> {
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

fn retry_delay_from_text(text: &str) -> Option<u64> {
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

fn rate_limit_cooldown_ms(
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

fn source_cooldown_ms(automatic_ms: u64, configured_ms: Option<u64>, explicit_hint: bool) -> u64 {
    configured_ms.map_or(automatic_ms, |configured| {
        if explicit_hint {
            automatic_ms.max(configured)
        } else {
            configured
        }
    })
}

pub(super) fn apply_cooldown(
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

pub(super) fn apply_cooldown_for_model(
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

pub(super) fn apply_cooldown_with_reason(
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
fn apply_cooldown_with_reason_for_model(
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

pub(super) fn apply_mandatory_cooldown(
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

pub(super) fn apply_failure_state(event: &mut UsageEvent, state: FailureState) {
    event.cooldown_scope = state.cooldown_scope;
    event.retry_at_ms = state.retry_at_ms;
    event.consecutive_failures = Some(state.consecutive_failures);
}

pub(super) fn retry_after_ms(headers: &reqwest::header::HeaderMap, now: SystemTime) -> Option<u64> {
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

fn exponential_backoff_ms(consecutive_failures: u32) -> u64 {
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    1_000_u64
        .saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX))
        .min(MAX_RATE_LIMIT_COOLDOWN_MS)
}

fn half_open_backoff_ms(duration_ms: u64, consecutive_failures: u32, half_open_probe: bool) -> u64 {
    if !half_open_probe {
        return duration_ms;
    }
    let duration_ms = duration_ms.max(1_000);
    let exponent = consecutive_failures.saturating_sub(1).min(31);
    duration_ms
        .saturating_mul(1_u64.checked_shl(exponent).unwrap_or(u64::MAX))
        .min(duration_ms.max(MAX_RATE_LIMIT_COOLDOWN_MS))
}

pub(super) fn cooldown_error(
    retry_at_ms: u64,
    failure: Option<&AttemptFailure>,
    all_sources_rate_limited: bool,
) -> Response<Body> {
    let seconds = retry_at_ms
        .saturating_sub(now_ms())
        .saturating_add(999)
        .checked_div(1_000)
        .unwrap_or_default()
        .max(1);
    let rate_limited = all_sources_rate_limited;
    let mut response = if rate_limited {
        failure
            .filter(|failure| failure.category == "upstream_quota_exhausted")
            .map_or_else(
                || {
                    api_error(
                        StatusCode::TOO_MANY_REQUESTS,
                        "all eligible sources are rate limited",
                        "all_sources_cooling_down",
                    )
                },
                |failure| api_error(failure.status, failure.message, failure.category),
            )
    } else {
        api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "all eligible sources are temporarily unavailable",
            "all_sources_temporarily_unavailable",
        )
    };
    if let Ok(value) = HeaderValue::from_str(&seconds.to_string()) {
        response.headers_mut().insert(RETRY_AFTER, value);
    }
    response
}

pub(super) fn api_error(status: StatusCode, message: &str, code: &str) -> Response<Body> {
    let code = api_error_code(code);
    let error_type = api_error_type(status, code);
    let mut response = (
        status,
        Json(json!({
            "error": {
                "message": message,
                "type": error_type,
                "code": code,
                "param": null,
            }
        })),
    )
        .into_response();
    response.extensions_mut().insert(LocalGatewayError);
    response
}

pub(super) fn api_error_type(status: StatusCode, code: &str) -> &'static str {
    if code == "insufficient_quota" {
        return "insufficient_quota";
    }
    match status {
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        status if status.is_server_error() => "server_error",
        _ => "invalid_request_error",
    }
}

pub(super) fn api_error_code(code: &str) -> &str {
    match code {
        "upstream_unauthorized" => "invalid_api_key",
        "upstream_account_disabled" => "account_deactivated",
        "upstream_account_verification_required" => "account_verification_required",
        "upstream_usage_not_included" => "usage_not_included",
        "upstream_quota_exhausted" => "insufficient_quota",
        "upstream_rate_limited" => "rate_limit_exceeded",
        "upstream_context_too_large" => "context_too_large",
        "upstream_encrypted_content_invalid" => "invalid_encrypted_content",
        "upstream_instructions_required" => "missing_required_parameter",
        "upstream_previous_response_not_found" => "previous_response_not_found",
        "upstream_tool_call_mismatch" => "tool_call_not_found",
        "upstream_content_policy" => "content_policy_violation",
        "upstream_payload_too_large" => "request_too_large",
        "upstream_unsupported_request" => "unsupported_request",
        "upstream_model_not_found" => "model_not_found",
        "upstream_model_unsupported" => "model_not_supported",
        "upstream_model_capacity" => "model_at_capacity",
        "upstream_websocket_unsupported" => "websocket_not_supported",
        "upstream_websocket_connection_limit" => "websocket_connection_limit_reached",
        "upstream_region_unsupported" => "unsupported_country_region_territory",
        "upstream_edge_challenge" => "edge_security_challenge",
        "upstream_forbidden" => "permission_denied",
        "upstream_not_found" => "not_found",
        "upstream_request_timeout" => "request_timeout",
        "upstream_conflict" => "conflict",
        "upstream_invalid_request" => "invalid_request",
        "upstream_overloaded" => "server_is_overloaded",
        "upstream_server_error" => "internal_server_error",
        "upstream_bad_gateway" => "bad_gateway",
        "upstream_unavailable" => "service_unavailable",
        "upstream_gateway_timeout" => "gateway_timeout",
        _ => code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn bad_request_affinity_recovery_requires_a_structured_missing_response_error() {
        for payload in [
            br#"{"error":{"code":"previous_response_not_found"}}"#.as_slice(),
            br#"{"message":"Previous response with id 'resp_123' not found."}"#.as_slice(),
        ] {
            assert!(recoverable_response_affinity_miss(
                StatusCode::BAD_REQUEST,
                true,
                false,
                previous_response_not_found(payload),
            ));
        }
        for payload in [
            br#"{"error":{"code":"invalid_request","message":"Invalid request body."}}"#.as_slice(),
            b"Previous response with id 'resp_123' not found.".as_slice(),
        ] {
            assert!(!recoverable_response_affinity_miss(
                StatusCode::BAD_REQUEST,
                true,
                false,
                previous_response_not_found(payload),
            ));
        }
        assert!(recoverable_response_affinity_miss(
            StatusCode::BAD_REQUEST,
            true,
            true,
            true,
        ));
        assert!(!recoverable_response_affinity_miss(
            StatusCode::BAD_REQUEST,
            true,
            true,
            false,
        ));
    }

    #[test]
    fn websocket_only_previous_response_errors_are_detected_without_matching_other_errors() {
        assert!(previous_response_requires_websocket(
            br#"{"error":{"message":"previous_response_id is only supported on Responses WebSocket v2"}}"#,
        ));
        assert!(!previous_response_requires_websocket(
            br#"{"error":{"message":"previous response with id resp_123 not found"}}"#,
        ));
        assert!(!previous_response_requires_websocket(
            br#"{"error":{"message":"WebSocket transport is unavailable"}}"#,
        ));
    }

    #[test]
    fn invalid_function_call_output_call_ids_are_detected_without_matching_generic_errors() {
        assert!(responses_function_call_output_has_invalid_call_id(
            br#"{"error":{"message":"Invalid call_id for function_call_output"}}"#,
        ));
        assert!(responses_function_call_output_has_invalid_call_id(
            br#"{"error":{"code":"invalid_function_call_output_call_id"}}"#,
        ));
        assert!(!responses_function_call_output_has_invalid_call_id(
            br#"{"error":{"message":"Invalid call_id"}}"#,
        ));
        assert!(!responses_function_call_output_has_invalid_call_id(
            br#"Invalid call_id for function_call_output"#,
        ));
    }

    #[test]
    fn strict_responses_function_item_id_error_is_detected_without_matching_call_id_errors() {
        assert!(responses_function_item_id_requires_fc_prefix(
            br#"{"error":{"message":"Invalid 'input[7].id': 'call_abc'. Expected an ID that begins with 'fc'."}}"#,
        ));
        assert!(!responses_function_item_id_requires_fc_prefix(
            br#"{"error":{"message":"Invalid call_id for function_call_output"}}"#,
        ));
        assert!(!responses_function_item_id_requires_fc_prefix(
            br#"{"error":{"message":"Expected an ID that begins with 'fc'."}}"#,
        ));
    }

    #[test]
    fn strict_responses_message_item_id_error_is_detected_without_matching_other_item_errors() {
        assert!(responses_message_item_id_requires_msg_prefix(
            br#"{"error":{"message":"Invalid 'input[151].id': 'item_abc'. Expected an ID that begins with 'msg'."}}"#,
        ));
        assert!(!responses_message_item_id_requires_msg_prefix(
            br#"{"error":{"message":"Invalid 'input[7].id': 'call_abc'. Expected an ID that begins with 'fc'."}}"#,
        ));
        assert!(!responses_message_item_id_requires_msg_prefix(
            br#"{"error":{"message":"Expected an ID that begins with 'msg'."}}"#,
        ));
    }

    #[test]
    fn upstream_errors_use_stable_status_and_body_categories() {
        let cases = [
            (
                StatusCode::UNAUTHORIZED,
                br#"{"error":{"code":"invalid_api_key"}}"#.as_slice(),
                "upstream_unauthorized",
            ),
            (
                StatusCode::FORBIDDEN,
                br#"{"error":{"code":"account_deactivated"}}"#.as_slice(),
                "upstream_account_disabled",
            ),
            (
                StatusCode::FORBIDDEN,
                br#"{"error":{"code":"phone_verification_required"}}"#.as_slice(),
                "upstream_account_verification_required",
            ),
            (
                StatusCode::PAYMENT_REQUIRED,
                br#"{"error":{"code":"deactivated_workspace"}}"#.as_slice(),
                "upstream_account_disabled",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"type":"usage_not_included"}}"#.as_slice(),
                "upstream_usage_not_included",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"type":"insufficient_quota"}}"#.as_slice(),
                "upstream_quota_exhausted",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"code":"rate_limit_exceeded"}}"#.as_slice(),
                "upstream_rate_limited",
            ),
            (
                StatusCode::NOT_FOUND,
                br#"{"error":{"code":"model_not_found"}}"#.as_slice(),
                "upstream_model_not_found",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"unsupported_parameter"}}"#.as_slice(),
                "upstream_unsupported_request",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"previous_response_not_found"}}"#.as_slice(),
                "upstream_previous_response_not_found",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"message":"No tool call found for custom tool call output with call_id call_1"}}"#.as_slice(),
                "upstream_tool_call_mismatch",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"message":"No tool output found for apply patch call call_1"}}"#.as_slice(),
                "upstream_tool_call_mismatch",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"context_length_exceeded"}}"#.as_slice(),
                "upstream_context_too_large",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"invalid_encrypted_content"}}"#.as_slice(),
                "upstream_encrypted_content_invalid",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"message":"Instructions are required"}}"#.as_slice(),
                "upstream_instructions_required",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"response":{"error":{"code":"invalid_prompt"}}}"#.as_slice(),
                "upstream_invalid_request",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"response":{"error":{"code":"bio_policy"}}}"#.as_slice(),
                "upstream_content_policy",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"model_at_capacity"}}"#.as_slice(),
                "upstream_model_capacity",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"token_invalidated"}}"#.as_slice(),
                "upstream_unauthorized",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"message":"An error occurred while processing your request"}}"#.as_slice(),
                "upstream_server_error",
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                br#"{"error":{"code":"server_is_overloaded"}}"#.as_slice(),
                "upstream_overloaded",
            ),
            (
                StatusCode::NOT_ACCEPTABLE,
                b"".as_slice(),
                "upstream_model_unsupported",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"invalid_request_error","message":"The 'gpt-next' model is not supported when using Codex with a ChatGPT account."}}"#.as_slice(),
                "upstream_model_unsupported",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"websocket_not_supported"}}"#.as_slice(),
                "upstream_websocket_unsupported",
            ),
            (
                StatusCode::PAYLOAD_TOO_LARGE,
                b"Failed to buffer request body: length limit exceeded".as_slice(),
                "upstream_payload_too_large",
            ),
            (
                StatusCode::FORBIDDEN,
                b"<!doctype html><title>Just a moment...</title>".as_slice(),
                "upstream_edge_challenge",
            ),
            (StatusCode::CONFLICT, b"".as_slice(), "upstream_conflict"),
            (
                StatusCode::from_u16(529).unwrap(),
                b"server overloaded".as_slice(),
                "upstream_overloaded",
            ),
        ];
        for (status, body, expected) in cases {
            assert_eq!(
                classify_upstream_error(status, Some(body)).category,
                expected,
                "status={status} body={}",
                String::from_utf8_lossy(body)
            );
        }
    }

    #[test]
    fn delayed_gateway_invalid_request_event_does_not_cool_down_source() {
        let value: Value = serde_json::from_slice(
            br#"{"type":"error","error":{"type":"invalid_request_error","code":"invalid_request","message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}"#,
        )
        .unwrap();

        let classification = classify_upstream_error_value(StatusCode::BAD_GATEWAY, &value);
        assert_eq!(classification.category, "upstream_invalid_request");
        assert_eq!(
            upstream_event_failure_category(Some("error"), &value),
            Some("upstream_invalid_request")
        );
        assert!(!failure_category_requires_cooldown(classification.category));
    }

    #[test]
    fn preserved_upstream_error_keeps_only_safe_structured_messages() {
        let failure = AttemptFailure::status_with_body(
            StatusCode::SERVICE_UNAVAILABLE,
            Some(
                br#"{"error":{"code":"service_unavailable","message":"no eligible source is available for this model"}}"#,
            ),
        );
        let preserved = preserved_upstream_error(
            &failure,
            br#"{"error":{"code":"service_unavailable","message":"no eligible source is available for this model"}}"#,
        )
        .expect("safe Gateway message is preserved");
        assert_eq!(preserved.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(preserved.category, "upstream_unavailable");
        assert_eq!(preserved.code, "service_unavailable");
        assert_eq!(
            preserved.message,
            "no eligible source is available for this model"
        );

        let nested = preserved_upstream_error(
            &AttemptFailure::classified_with_hint(
                StatusCode::BAD_REQUEST,
                "upstream_invalid_request",
                RateLimitBodyHint::default(),
            ),
            br#"{"type":"error","response":{"error":{"code":"bad_request","message":"Zenith AI request is invalid."}}}"#,
        )
        .expect("safe nested Gateway message is preserved");
        assert_eq!(nested.code, "bad_request");
        assert_eq!(nested.message, "Zenith AI request is invalid.");

        assert!(preserved_upstream_error(
            &failure,
            br#"{"error":{"message":"request failed at https://gateway.example.invalid/v1; bearer secret"}}"#,
        )
        .is_none());
        assert!(preserved_upstream_error(
            &failure,
            br#"{"error":{"code":"provider_error","message":"upstream diagnostic"}}"#,
        )
        .is_none());
    }

    #[test]
    fn retry_policy_matches_account_failover_and_official_transient_statuses() {
        assert!(retryable_status(StatusCode::UNAUTHORIZED, false));
        assert!(retryable_status(StatusCode::CONFLICT, false));
        assert!(retryable_status(StatusCode::from_u16(529).unwrap(), false));
        assert!(!retryable_status(StatusCode::PAYLOAD_TOO_LARGE, false));
        assert!(!retryable_status(StatusCode::BAD_REQUEST, false));
        assert!(retryable_failure(
            StatusCode::BAD_REQUEST,
            "upstream_model_capacity",
            false
        ));
        assert!(retryable_failure(
            StatusCode::BAD_REQUEST,
            "upstream_overloaded",
            false
        ));
        assert!(retryable_failure(
            StatusCode::BAD_GATEWAY,
            "upstream_usage_not_included",
            false
        ));
        assert!(!retryable_failure(
            StatusCode::BAD_REQUEST,
            "upstream_context_too_large",
            false
        ));
        assert!(!retryable_failure(
            StatusCode::FORBIDDEN,
            "upstream_content_policy",
            false
        ));
        for category in [
            "upstream_stream",
            "stream_incomplete",
            "stream_idle_timeout",
            "upstream_invalid_request",
        ] {
            assert!(!failure_category_requires_cooldown(category));
        }
        assert_eq!(
            AttemptFailure::status_with_body(
                StatusCode::BAD_REQUEST,
                Some(br#"{"error":{"code":"model_at_capacity"}}"#)
            )
            .status,
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            canonical_upstream_status(StatusCode::FORBIDDEN, "upstream_quota_exhausted"),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            canonical_upstream_status(StatusCode::TOO_MANY_REQUESTS, "upstream_usage_not_included"),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn local_errors_use_openai_compatible_error_types() {
        assert_eq!(
            api_error_type(StatusCode::UNAUTHORIZED, "invalid_api_key"),
            "authentication_error"
        );
        assert_eq!(
            api_error_type(StatusCode::FORBIDDEN, "permission_denied"),
            "permission_error"
        );
        assert_eq!(
            api_error_type(StatusCode::TOO_MANY_REQUESTS, "rate_limit_exceeded"),
            "rate_limit_error"
        );
        assert_eq!(
            api_error_type(StatusCode::BAD_REQUEST, "invalid_request"),
            "invalid_request_error"
        );
        assert_eq!(
            api_error_type(StatusCode::BAD_GATEWAY, "bad_gateway"),
            "server_error"
        );
        assert_eq!(
            api_error_type(StatusCode::TOO_MANY_REQUESTS, "insufficient_quota"),
            "insufficient_quota"
        );
        assert_eq!(
            api_error_code("upstream_quota_exhausted"),
            "insufficient_quota"
        );
        assert_eq!(
            api_error_code("upstream_usage_not_included"),
            "usage_not_included"
        );
        assert_eq!(
            api_error_code("upstream_model_capacity"),
            "model_at_capacity"
        );
        assert_eq!(api_error_code("local_internal_code"), "local_internal_code");
    }

    #[tokio::test]
    async fn exhausted_quota_survives_the_cooldown_response_shape() {
        let failure = AttemptFailure::status_with_body(
            StatusCode::TOO_MANY_REQUESTS,
            Some(br#"{"error":{"type":"insufficient_quota"}}"#),
        );
        let response = cooldown_error(now_ms().saturating_add(60_000), Some(&failure), true);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key(RETRY_AFTER));

        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value.pointer("/error/type").unwrap(), "insufficient_quota");
        assert_eq!(value.pointer("/error/code").unwrap(), "insufficient_quota");
        assert!(value.pointer("/error/param").unwrap().is_null());
    }

    #[tokio::test]
    async fn transient_cooldown_is_not_reported_as_rate_limit() {
        let failure = AttemptFailure::status_with_body(
            StatusCode::BAD_GATEWAY,
            Some(br#"{"error":{"message":"upstream unavailable"}}"#),
        );
        let response = cooldown_error(now_ms().saturating_add(60_000), Some(&failure), false);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()[RETRY_AFTER], "60");

        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.pointer("/error/code").unwrap(),
            "all_sources_temporarily_unavailable"
        );
    }

    #[tokio::test]
    async fn mixed_cooldowns_are_not_reported_as_rate_limit() {
        let failure = AttemptFailure::status_with_body(StatusCode::TOO_MANY_REQUESTS, None);
        let response = cooldown_error(now_ms().saturating_add(60_000), Some(&failure), false);
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            value.pointer("/error/code").unwrap(),
            "all_sources_temporarily_unavailable"
        );
    }

    #[test]
    fn retry_after_supports_delta_seconds_and_http_dates() {
        let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(RETRY_AFTER, reqwest::header::HeaderValue::from_static("17"));
        assert_eq!(retry_after_ms(&headers, now), Some(17_000));

        headers.insert(
            RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("518400"),
        );
        assert_eq!(retry_after_ms(&headers, now), Some(518_400_000));

        let date = httpdate::fmt_http_date(now + Duration::from_secs(23));
        headers.insert(RETRY_AFTER, date.parse().unwrap());
        assert_eq!(retry_after_ms(&headers, now), Some(23_000));
    }

    #[test]
    fn rate_limit_body_hint_uses_reset_time_and_marks_usage_limits_global() {
        let hint = rate_limit_body_hint_at(
            br#"{"error":{"type":"usage_limit_reached","resets_at":1700000120,"resets_in_seconds":1}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(hint.retry_after_ms, Some(120_000));
        assert!(hint.global);
    }

    #[test]
    fn rate_limit_body_hint_accepts_relative_reset_seconds() {
        let hint = rate_limit_body_hint_at(
            br#"{"error":{"code":"rate_limit","resets_in_seconds":"17"}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(hint.retry_after_ms, Some(17_000));
        assert!(!hint.global);
    }

    #[test]
    fn rate_limit_body_hint_accepts_retry_after_and_message_delays() {
        let retry_after = rate_limit_body_hint_at(
            br#"{"error":{"code":"rate_limit_exceeded","retry_after":"2.5"}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(retry_after.retry_after_ms, Some(2_500));

        let seconds = rate_limit_body_hint_at(
            br#"{"response":{"error":{"code":"rate_limit_exceeded","message":"Please try again in 11.054s."}}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(seconds.retry_after_ms, Some(11_054));

        let millis = rate_limit_body_hint_at(
            br#"{"error":{"message":"Please try again in 250ms."}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(millis.retry_after_ms, Some(250));
    }

    #[test]
    fn rate_limit_body_hint_accepts_top_level_quota_variants() {
        let hint = rate_limit_body_hint_at(
            br#"{"code":"rate_limit_reached","resets_in_seconds":9}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert_eq!(hint.retry_after_ms, Some(9_000));
        assert!(hint.global);
    }

    #[test]
    fn websocket_connection_limit_is_account_global() {
        let hint = rate_limit_body_hint_at(
            br#"{"error":{"code":"websocket_connection_limit_reached"}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
        assert!(hint.global);
    }

    #[test]
    fn rate_limit_delay_uses_the_stronger_hint_and_keeps_explicit_zero() {
        assert_eq!(
            rate_limit_cooldown_ms(Some(1_000), Some(120_000), 1),
            120_000
        );
        assert_eq!(rate_limit_cooldown_ms(Some(0), None, 5), 0);
    }

    #[test]
    fn source_recovery_delay_overrides_automatic_but_not_provider_retry_after() {
        assert_eq!(source_cooldown_ms(5_000, Some(60_000), false), 60_000);
        assert_eq!(source_cooldown_ms(120_000, Some(60_000), true), 120_000);
        assert_eq!(source_cooldown_ms(5_000, None, false), 5_000);
    }

    #[test]
    fn no_header_rate_limit_backoff_is_exponential_and_capped() {
        assert_eq!(exponential_backoff_ms(1), 1_000);
        assert_eq!(exponential_backoff_ms(2), 2_000);
        assert_eq!(exponential_backoff_ms(3), 4_000);
        assert_eq!(exponential_backoff_ms(32), MAX_RATE_LIMIT_COOLDOWN_MS);
    }

    #[test]
    fn failed_half_open_probes_back_off_without_shortening_retry_after() {
        assert_eq!(half_open_backoff_ms(0, 2, true), 2_000);
        assert_eq!(half_open_backoff_ms(60_000, 2, false), 60_000);
        assert_eq!(half_open_backoff_ms(60_000, 2, true), 120_000);
        assert_eq!(half_open_backoff_ms(60_000, 3, true), 240_000);
        assert_eq!(
            half_open_backoff_ms(60_000, 32, true),
            MAX_RATE_LIMIT_COOLDOWN_MS
        );
        assert_eq!(
            half_open_backoff_ms(MAX_RATE_LIMIT_RETRY_HINT_MS, 2, true),
            MAX_RATE_LIMIT_RETRY_HINT_MS
        );
    }
}
