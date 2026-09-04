use super::*;
use crate::ErrorOrigin;

pub(crate) fn cooldown_error(
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

pub(crate) fn api_error(status: StatusCode, message: &str, code: &str) -> Response<Body> {
    api_error_with_origin(status, message, code, ErrorOrigin::Relay, None)
}

pub(crate) fn api_error_with_origin(
    status: StatusCode,
    message: &str,
    code: &str,
    origin: ErrorOrigin,
    request_id: Option<&str>,
) -> Response<Body> {
    api_error_with_origin_and_category(status, message, code, code, origin, request_id)
}

pub(crate) fn api_error_with_origin_and_category(
    status: StatusCode,
    message: &str,
    code: &str,
    category: &str,
    origin: ErrorOrigin,
    request_id: Option<&str>,
) -> Response<Body> {
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
                "zenith_relay": {
                    "origin": origin.as_str(),
                    "category": category,
                    "request_id": request_id,
                },
            }
        })),
    )
        .into_response();
    super::super::response::attach_error_diagnostics(&mut response, origin, category, request_id);
    response.extensions_mut().insert(LocalGatewayError);
    response
}

pub(crate) fn api_error_type(status: StatusCode, code: &str) -> &'static str {
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

pub(crate) fn api_error_code(code: &str) -> &str {
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
        "upstream_candidate_rejected" => "source_rejected",
        "upstream_overloaded" => "server_is_overloaded",
        "upstream_server_error" => "internal_server_error",
        "upstream_bad_gateway" => "bad_gateway",
        "upstream_unavailable" => "service_unavailable",
        "upstream_gateway_timeout" => "gateway_timeout",
        _ => code,
    }
}
