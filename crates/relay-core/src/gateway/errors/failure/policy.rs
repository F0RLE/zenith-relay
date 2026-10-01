use super::*;

pub(crate) fn retryable_status(status: StatusCode, has_previous_response_id: bool) -> bool {
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

pub(crate) fn retryable_failure(
    status: StatusCode,
    category: &str,
    has_previous_response_id: bool,
) -> bool {
    if !failure_category_requires_cooldown(category) {
        return false;
    }
    if category == error_codes::UPSTREAM_CANDIDATE_REJECTED {
        // Selection preserves opaque ownership; only a complete local replay
        // can release it. A generic rejection must not prevent that recovery.
        return true;
    }
    retryable_status(status, has_previous_response_id)
        || matches!(
            category,
            error_codes::UPSTREAM_UNAUTHORIZED
                | error_codes::UPSTREAM_ACCOUNT_DISABLED
                | error_codes::UPSTREAM_ACCOUNT_VERIFICATION_REQUIRED
                | error_codes::UPSTREAM_FORBIDDEN
                | error_codes::UPSTREAM_REGION_UNSUPPORTED
                | error_codes::UPSTREAM_MODEL_NOT_FOUND
                | error_codes::UPSTREAM_ROUTE_DEGRADED
                | error_codes::UPSTREAM_MODEL_UNAVAILABLE
                | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
                | error_codes::UPSTREAM_QUOTA_EXHAUSTED
                | error_codes::UPSTREAM_MODEL_UNSUPPORTED
                | error_codes::UPSTREAM_MODEL_CAPACITY
                | error_codes::UPSTREAM_WEBSOCKET_CONNECTION_LIMIT
                | error_codes::UPSTREAM_RATE_LIMITED
                | error_codes::UPSTREAM_REFRESH_TOKEN_REUSED
                | error_codes::UPSTREAM_REQUEST_TIMEOUT
                | error_codes::UPSTREAM_OVERLOADED
                | error_codes::UPSTREAM_EDGE_CHALLENGE
                | error_codes::UPSTREAM_SERVER_ERROR
                | error_codes::UPSTREAM_BAD_GATEWAY
                | error_codes::UPSTREAM_UNAVAILABLE
                | error_codes::UPSTREAM_GATEWAY_TIMEOUT
                | error_codes::UPSTREAM_TRANSPORT_TIMEOUT
                | error_codes::UPSTREAM_TRANSPORT_CONNECT
                | error_codes::UPSTREAM_TRANSPORT_BODY
                | error_codes::UPSTREAM_TRANSPORT_REQUEST
                | error_codes::UPSTREAM_TRANSPORT
                | error_codes::UPSTREAM_ERROR
        )
}

pub(crate) fn failure_category_requires_cooldown(category: &str) -> bool {
    !failure_category_is_request_terminal(category)
        && !matches!(
            category,
            error_codes::CLIENT_CANCELLED
                | error_codes::RESPONSE_AFFINITY_MISS
                | error_codes::RESPONSE_INCOMPLETE
                | error_codes::UPSTREAM_CANCELLED
                | error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
        )
}

/// A terminal or cancelled request does not cool the route. Capacity, overload,
/// and gateway failures may cool the route without marking the account unavailable.
pub(crate) fn failure_category_affects_account_state(category: &str) -> bool {
    failure_category_requires_cooldown(category)
        && !matches!(
            category,
            error_codes::UPSTREAM_MODEL_NOT_FOUND
                | error_codes::UPSTREAM_MODEL_UNSUPPORTED
                | error_codes::UPSTREAM_USAGE_NOT_INCLUDED
                | error_codes::UPSTREAM_MODEL_CAPACITY
                | error_codes::UPSTREAM_OVERLOADED
                | error_codes::UPSTREAM_SERVER_ERROR
                | error_codes::UPSTREAM_BAD_GATEWAY
                | error_codes::UPSTREAM_UNAVAILABLE
                | error_codes::UPSTREAM_GATEWAY_TIMEOUT
                | error_codes::IMAGE_GENERATION_NOT_ENABLED
        )
}

pub(crate) fn failure_category_is_request_terminal(category: &str) -> bool {
    matches!(
        category,
        error_codes::UPSTREAM_TOOL_CALL_MISMATCH
            | error_codes::UPSTREAM_CONTEXT_TOO_LARGE
            | error_codes::UPSTREAM_ENCRYPTED_CONTENT_INVALID
            | error_codes::UPSTREAM_INSTRUCTIONS_REQUIRED
            | error_codes::UPSTREAM_CONTENT_POLICY
            | error_codes::UPSTREAM_PAYLOAD_TOO_LARGE
            | error_codes::UPSTREAM_UNSUPPORTED_REQUEST
            | error_codes::UPSTREAM_WEBSOCKET_UNSUPPORTED
            | error_codes::UPSTREAM_INVALID_REQUEST
    )
}
