use super::*;

pub(super) fn classify_upstream_error_text(
    status: StatusCode,
    text: &str,
) -> UpstreamErrorClassification {
    let category = RULES
        .iter()
        .find_map(|rule| rule.classify(status, text))
        .unwrap_or(error_codes::UPSTREAM_STATUS);
    UpstreamErrorClassification {
        category,
        message: upstream_failure_message(category),
    }
}

enum Rule {
    Phrases(&'static [&'static str], &'static str),
    Status(StatusCode, &'static str),
    Custom(fn(StatusCode, &str) -> Option<&'static str>),
}

impl Rule {
    fn classify(&self, status: StatusCode, text: &str) -> Option<&'static str> {
        match self {
            Self::Phrases(phrases, category) => text_has_any(text, phrases).then_some(*category),
            Self::Status(expected, category) => (status == *expected).then_some(*category),
            Self::Custom(classify) => classify(status, text),
        }
    }
}

const RULES: &[Rule] = &[
    Rule::Phrases(
        &[error_codes::UPSTREAM_ROUTE_DEGRADED],
        error_codes::UPSTREAM_ROUTE_DEGRADED,
    ),
    Rule::Phrases(
        &[
            error_codes::RESPONSE_CONTINUATION_UNAVAILABLE,
            "responses continuation route is unknown",
            "responses continuation is bound to a provider slot",
            "responses continuation requires the same native provider endpoint",
        ],
        error_codes::RESPONSE_AFFINITY_MISS,
    ),
    // A refresh-token race is transient. The token authority retries it
    // without changing account authentication state, and a proxied
    // upstream response must follow the same rule.
    Rule::Phrases(
        &[error_codes::REFRESH_TOKEN_REUSED],
        error_codes::UPSTREAM_REFRESH_TOKEN_REUSED,
    ),
    Rule::Phrases(
        &[
            "previous_response_not_found",
            "invalid_previous_response_id",
            "previous response not found",
            "no response found for previous_response_id",
            "unknown or expired previous_response_id",
        ],
        error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND,
    ),
    Rule::Phrases(
        &[
            "phone_verification_required",
            "phone number verification required",
            "verify your phone number",
            "account_verification_required",
            "account verification required",
            "verify your account",
            "account must be verified",
        ],
        error_codes::UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED,
    ),
    Rule::Custom(tool_call_mismatch),
    Rule::Custom(context_too_large),
    Rule::Phrases(
        &[
            "invalid_encrypted_content",
            "thinking_signature_invalid",
            "invalid signature in thinking block",
            "encrypted content could not be verified",
            "encrypted content could not be decrypted or parsed",
        ],
        error_codes::UPSTREAM_ENCRYPTED_CONTENT_INVALID,
    ),
    Rule::Phrases(
        &[
            "instructions are required",
            "required parameter: 'instructions'",
            "required parameter: instructions",
        ],
        error_codes::UPSTREAM_INSTRUCTIONS_REQUIRED,
    ),
    Rule::Phrases(
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
        error_codes::UPSTREAM_ACCOUNT_DISABLED,
    ),
    Rule::Phrases(
        &[
            "usage_not_included",
            "not included in your plan",
            "subscription does not include",
        ],
        error_codes::UPSTREAM_USAGE_NOT_INCLUDED,
    ),
    // Some hosts wrap these 403 responses in `insufficient_quota`.
    // The specific provider refusal takes precedence over that wrapper.
    Rule::Phrases(
        &["blocked by our usage policy"],
        error_codes::UPSTREAM_CONTENT_POLICY,
    ),
    Rule::Phrases(
        &["model access has changed"],
        error_codes::UPSTREAM_MODEL_UNAVAILABLE,
    ),
    Rule::Custom(quota_exhausted),
    Rule::Custom(unauthorized),
    Rule::Phrases(
        &[
            "unsupported_country_region_territory",
            "country_not_supported",
            "region_not_supported",
            "country, region, or territory not supported",
        ],
        error_codes::UPSTREAM_REGION_UNSUPPORTED,
    ),
    Rule::Phrases(
        &[
            "content_policy_violation",
            "content_filter",
            "policy_violation",
            "safety_violation",
            "cyber_policy",
            "bio_policy",
            "content_moderation_failed",
        ],
        error_codes::UPSTREAM_CONTENT_POLICY,
    ),
    // An explicit route rejection is scoped to that model, not a generic
    // invalid input and not a vote against the provider's inference health.
    Rule::Phrases(
        &[
            "model_disabled",
            "requested model is disabled",
            "this route cannot serve the request",
        ],
        error_codes::UPSTREAM_CANDIDATE_REJECTED,
    ),
    Rule::Custom(invalid_request),
    Rule::Custom(payload_too_large),
    Rule::Phrases(
        &[
            "unsupported_parameter",
            error_codes::UNSUPPORTED_VALUE,
            "invalid_parameter",
            "parameter_not_supported",
        ],
        error_codes::UPSTREAM_UNSUPPORTED_REQUEST,
    ),
    Rule::Phrases(
        &[
            "model_at_capacity",
            "selected model is at capacity",
            "model is at capacity",
        ],
        error_codes::UPSTREAM_MODEL_CAPACITY,
    ),
    Rule::Phrases(
        &["model_not_available"],
        error_codes::UPSTREAM_MODEL_UNAVAILABLE,
    ),
    Rule::Custom(degraded_route_model),
    Rule::Phrases(
        &[error_codes::MODEL_NOT_FOUND],
        error_codes::UPSTREAM_MODEL_NOT_FOUND,
    ),
    Rule::Custom(model_unsupported),
    Rule::Custom(websocket_unsupported),
    Rule::Phrases(
        &["websocket_connection_limit_reached"],
        error_codes::UPSTREAM_WEBSOCKET_CONNECTION_LIMIT,
    ),
    Rule::Phrases(
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
        error_codes::UPSTREAM_RATE_LIMITED,
    ),
    Rule::Custom(overloaded),
    Rule::Phrases(
        &["service_unavailable", "temporarily unavailable"],
        error_codes::UPSTREAM_UNAVAILABLE,
    ),
    Rule::Custom(server_error_text),
    Rule::Custom(edge_challenge),
    Rule::Status(StatusCode::FORBIDDEN, error_codes::UPSTREAM_FORBIDDEN),
    Rule::Status(StatusCode::NOT_FOUND, error_codes::UPSTREAM_NOT_FOUND),
    Rule::Status(
        StatusCode::REQUEST_TIMEOUT,
        error_codes::UPSTREAM_REQUEST_TIMEOUT,
    ),
    Rule::Status(StatusCode::CONFLICT, error_codes::UPSTREAM_CONFLICT),
    Rule::Status(
        StatusCode::TOO_MANY_REQUESTS,
        error_codes::UPSTREAM_RATE_LIMITED,
    ),
    Rule::Status(
        StatusCode::INTERNAL_SERVER_ERROR,
        error_codes::UPSTREAM_SERVER_ERROR,
    ),
    Rule::Status(StatusCode::BAD_GATEWAY, error_codes::UPSTREAM_BAD_GATEWAY),
    Rule::Status(
        StatusCode::SERVICE_UNAVAILABLE,
        error_codes::UPSTREAM_UNAVAILABLE,
    ),
    Rule::Status(
        StatusCode::GATEWAY_TIMEOUT,
        error_codes::UPSTREAM_GATEWAY_TIMEOUT,
    ),
    Rule::Custom(client_error),
    Rule::Custom(server_error),
];

fn tool_call_mismatch(_status: StatusCode, text: &str) -> Option<&'static str> {
    (text_has_any(
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
    ) || super::super::failure::responses_call_id_is_missing_text(text))
    .then_some(error_codes::UPSTREAM_TOOL_CALL_MISMATCH)
}

fn degraded_route_model(_status: StatusCode, text: &str) -> Option<&'static str> {
    crate::is_degraded_route_model(text).then_some(error_codes::UPSTREAM_ROUTE_DEGRADED)
}

fn context_too_large(_status: StatusCode, text: &str) -> Option<&'static str> {
    (text_has_any(
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
            && text_has_any(text, &["exceed", "too large", "too long"])))
    .then_some(error_codes::UPSTREAM_CONTEXT_TOO_LARGE)
}

fn quota_exhausted(status: StatusCode, text: &str) -> Option<&'static str> {
    (text_has_any(
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
    ) || status == StatusCode::PAYMENT_REQUIRED)
        .then_some(error_codes::UPSTREAM_QUOTA_EXHAUSTED)
}

fn unauthorized(status: StatusCode, text: &str) -> Option<&'static str> {
    (text_has_any(
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
    ) || status == StatusCode::UNAUTHORIZED)
        .then_some(error_codes::UPSTREAM_UNAUTHORIZED)
}

fn invalid_request(_status: StatusCode, text: &str) -> Option<&'static str> {
    (text_has_any(
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
        )))
    .then_some(error_codes::UPSTREAM_INVALID_REQUEST)
}

fn payload_too_large(status: StatusCode, text: &str) -> Option<&'static str> {
    (status == StatusCode::PAYLOAD_TOO_LARGE
        || text_has_any(
            text,
            &[
                error_codes::REQUEST_TOO_LARGE,
                "payload_too_large",
                "content_too_large",
                "request body too large",
                "length limit exceeded",
            ],
        ))
    .then_some(error_codes::UPSTREAM_PAYLOAD_TOO_LARGE)
}

fn model_unsupported(status: StatusCode, text: &str) -> Option<&'static str> {
    (status == StatusCode::NOT_ACCEPTABLE
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
            && text.contains("does not exist or you do not have access to it")))
    .then_some(error_codes::UPSTREAM_MODEL_UNSUPPORTED)
}

fn websocket_unsupported(status: StatusCode, text: &str) -> Option<&'static str> {
    (status == StatusCode::UPGRADE_REQUIRED
        || text_has_any(text, &["websocket_not_supported", "websocket_unsupported"]))
    .then_some(error_codes::UPSTREAM_WEBSOCKET_UNSUPPORTED)
}

fn overloaded(status: StatusCode, text: &str) -> Option<&'static str> {
    (status.as_u16() == 529
        || text_has_any(
            text,
            &["server_is_overloaded", "server_overloaded", "overloaded"],
        ))
    .then_some(error_codes::UPSTREAM_OVERLOADED)
}

fn server_error_text(_status: StatusCode, text: &str) -> Option<&'static str> {
    (text_has_any(
        text,
        &[
            "internal_server_error",
            "server_error",
            "an error occurred while processing your request",
        ],
    ) || (text.contains("you can retry your request") && text.contains("request id")))
    .then_some(error_codes::UPSTREAM_SERVER_ERROR)
}

fn edge_challenge(status: StatusCode, text: &str) -> Option<&'static str> {
    (status == StatusCode::FORBIDDEN
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
        ))
    .then_some(error_codes::UPSTREAM_EDGE_CHALLENGE)
}

fn client_error(status: StatusCode, _text: &str) -> Option<&'static str> {
    status
        .is_client_error()
        .then_some(error_codes::UPSTREAM_INVALID_REQUEST)
}

fn server_error(status: StatusCode, _text: &str) -> Option<&'static str> {
    status
        .is_server_error()
        .then_some(error_codes::UPSTREAM_SERVER_ERROR)
}
