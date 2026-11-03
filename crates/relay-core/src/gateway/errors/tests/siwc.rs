use super::*;

#[test]
fn siwc_structured_errors_take_precedence_over_diagnostic_prose() {
    for (code, status, category) in [
        (
            "subscription_sharing_user_not_eligible",
            StatusCode::FORBIDDEN,
            error_codes::UPSTREAM_SIWC_NOT_ELIGIBLE,
        ),
        (
            "subscription_sharing_usage_limit_exceeded",
            StatusCode::TOO_MANY_REQUESTS,
            error_codes::UPSTREAM_QUOTA_EXHAUSTED,
        ),
        (
            "subscription_sharing_usage_unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
        ),
        (
            "subscription_sharing_user_unavailable",
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
        ),
        (
            "subscription_sharing_unsupported_capability",
            StatusCode::BAD_REQUEST,
            error_codes::UPSTREAM_UNSUPPORTED_REQUEST,
        ),
        (
            "subscription_sharing_invalid_user",
            StatusCode::UNAUTHORIZED,
            error_codes::UPSTREAM_UNAUTHORIZED,
        ),
        (
            "subscription_sharing_route_not_supported",
            StatusCode::FORBIDDEN,
            error_codes::UPSTREAM_FORBIDDEN,
        ),
        (
            "chatpass_v2_scope_not_authorized",
            StatusCode::FORBIDDEN,
            error_codes::UPSTREAM_FORBIDDEN,
        ),
        (
            "chatpass_v2_invalid_authorization_context",
            StatusCode::FORBIDDEN,
            error_codes::UPSTREAM_FORBIDDEN,
        ),
    ] {
        for field in ["code", "type"] {
            let mut error = json!({
                "message":"Synthetic diagnostics: invalid_grant insufficient_quota account_deactivated",
                "param":"tools"
            });
            error[field] = json!(code);
            for body in [
                error.clone(),
                json!({"error":error}),
                json!({"response":{"error":error}}),
                json!({"body":{"error":error}}),
                json!({"error":{"upstream_error":error}}),
            ] {
                let failure = AttemptFailure::status_with_body(
                    status,
                    Some(&serde_json::to_vec(&body).unwrap()),
                );
                assert_eq!(failure.category, category, "{code}: {body}");
                assert_eq!(failure.status, status, "{code}");
                assert_eq!(
                    failure.cooldown_hint.siwc_usage_limit,
                    code == "subscription_sharing_usage_limit_exceeded"
                );
            }
            let frame = format!(
                "data: {}\n\n",
                json!({"type":"response.failed","response":{"status":"failed","error":error}})
            );
            let event = crate::gateway::streaming::parse_sse_event(frame.as_bytes());
            assert_eq!(event.error_category, Some(category), "{code}");
            assert_eq!(event.error_status, Some(status), "{code}");
        }
    }
}

#[test]
fn siwc_quota_proof_excludes_prose_native_codes_and_echoed_inputs() {
    let code = "subscription_sharing_usage_limit_exceeded";
    for body in [
        json!({"error":{"message":code}}),
        json!({"detail":code}),
        json!({"input":{"code":code}}),
        json!({"error":{"param":code}}),
        json!({"error":{"code":"insufficient_quota"}}),
        json!({"error":{"type":"usage_limit_reached"}}),
        json!({"error":{"code":"subscription_sharing_usage_unavailable","upstream_error":{"code":code}}}),
    ] {
        let failure = AttemptFailure::status_with_body(
            StatusCode::TOO_MANY_REQUESTS,
            Some(&serde_json::to_vec(&body).unwrap()),
        );
        assert!(!failure.cooldown_hint.siwc_usage_limit, "{body}");
    }
}

#[test]
fn siwc_unknown_codes_do_not_infer_auth_or_quota_from_messages() {
    for code in [
        "subscription_sharing_future_error",
        "chatpass_v2_future_error",
    ] {
        let body = json!({"error":{
            "code":code,"message":"Synthetic invalid_grant insufficient_quota account_deactivated"
        }});
        let failure = AttemptFailure::status_with_body(
            StatusCode::SERVICE_UNAVAILABLE,
            Some(&serde_json::to_vec(&body).unwrap()),
        );
        assert_eq!(failure.category, error_codes::UPSTREAM_UNAVAILABLE);
        assert_eq!(
            failure.execution.certainty,
            crate::scheduler::rotation::ExecutionCertainty::Unknown
        );
        assert!(!failure_category_affects_account_state(failure.category));
        assert!(!failure.cooldown_hint.siwc_usage_limit);
    }
}

#[test]
fn siwc_quota_code_does_not_override_conflicting_http_status() {
    for (status, expected) in [
        (StatusCode::UNAUTHORIZED, error_codes::UPSTREAM_UNAUTHORIZED),
        (StatusCode::FORBIDDEN, error_codes::UPSTREAM_FORBIDDEN),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
        ),
    ] {
        let failure = AttemptFailure::status_with_body(
            status,
            Some(br#"{"error":{"code":"subscription_sharing_usage_limit_exceeded"}}"#),
        );
        assert_eq!(failure.category, expected);
        assert_eq!(failure.status, status);
    }
}
