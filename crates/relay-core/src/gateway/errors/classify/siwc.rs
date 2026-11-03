use super::*;

/// SIWC error messages are diagnostics, not stable policy discriminators.
pub(in crate::gateway::errors) fn error_code(payload: &Value) -> Option<&str> {
    let error = ["/error", "/response/error", "/body/error"]
        .into_iter()
        .find_map(|path| payload.pointer(path))
        .unwrap_or(payload);
    // Presence establishes precedence. An unknown or malformed discriminator
    // must not borrow a retry permission from another field or error envelope.
    let code = match error.get("code").or_else(|| error.get("type")) {
        Some(code) => code,
        None => {
            let upstream = error.get("upstream_error")?;
            upstream.get("code").or_else(|| upstream.get("type"))?
        }
    };
    code.as_str().filter(|code| {
        code.starts_with("subscription_sharing_") || code.starts_with("chatpass_v2_")
    })
}

pub(super) fn category(status: StatusCode, payload: &Value) -> Option<&'static str> {
    let (expected, category) = match error_code(payload)? {
        "subscription_sharing_user_not_eligible" => (
            StatusCode::FORBIDDEN,
            error_codes::UPSTREAM_SIWC_NOT_ELIGIBLE,
        ),
        "subscription_sharing_usage_limit_exceeded" => (
            StatusCode::TOO_MANY_REQUESTS,
            error_codes::UPSTREAM_QUOTA_EXHAUSTED,
        ),
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => (
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
        ),
        "subscription_sharing_unsupported_capability" => (
            StatusCode::BAD_REQUEST,
            error_codes::UPSTREAM_UNSUPPORTED_REQUEST,
        ),
        "subscription_sharing_invalid_user" => {
            (StatusCode::UNAUTHORIZED, error_codes::UPSTREAM_UNAUTHORIZED)
        }
        "subscription_sharing_route_not_supported"
        | "chatpass_v2_scope_not_authorized"
        | "chatpass_v2_invalid_authorization_context" => {
            (StatusCode::FORBIDDEN, error_codes::UPSTREAM_FORBIDDEN)
        }
        // Future codes keep their actual status, without interpreting their prose.
        _ => return Some(classify_upstream_error_text(status, "").category),
    };
    Some(if status == expected || status == StatusCode::BAD_GATEWAY {
        // SSE error events and compatible proxy envelopes use 502 as the
        // classification input when they have no explicit upstream status.
        category
    } else {
        classify_upstream_error_text(status, "").category
    })
}
