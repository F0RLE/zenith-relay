use super::*;

#[test]
fn relay_capacity_code_survives_generic_text_and_another_relay_hop() {
    let error = serde_json::json!({
        "code":"upstream_capacity_exhausted",
        "type":"server_error",
        "message":"Temporarily unavailable"
    });
    for body in [
        error.clone(),
        serde_json::json!({"error":error}),
        serde_json::json!({"response":{"error":error}}),
        serde_json::json!({"body":{"error":error}}),
    ] {
        let first = AttemptFailure::status_with_body(
            StatusCode::SERVICE_UNAVAILABLE,
            Some(&serde_json::to_vec(&body).unwrap()),
        );
        assert_eq!(first.category, error_codes::UPSTREAM_MODEL_CAPACITY);
        assert_eq!(first.status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(!failure_category_affects_account_state(first.category));
        assert_eq!(
            first.execution.certainty,
            crate::scheduler::rotation::ExecutionCertainty::NotSent
        );
        let forwarded = serde_json::json!({"error":{
            "code":error_codes::public_code(first.category),"message":first.message
        }});
        let second = AttemptFailure::status_with_body(
            first.status,
            Some(&serde_json::to_vec(&forwarded).unwrap()),
        );
        assert_eq!(second.category, first.category);
        assert!(!failure_category_affects_account_state(second.category));
    }
    let frame = format!(
        "data: {}\n\n",
        serde_json::json!({
            "type":"response.failed","response":{"status":"failed","error":error}
        })
    );
    let terminal = crate::gateway::streaming::parse_sse_event(frame.as_bytes());
    assert_eq!(
        terminal.error_category,
        Some(error_codes::UPSTREAM_MODEL_CAPACITY)
    );
    assert_eq!(terminal.error_status, Some(StatusCode::SERVICE_UNAVAILABLE));
    assert!(!terminal.semantic_output);
}

#[test]
fn relay_capacity_code_cannot_override_auth_quota_or_policy() {
    for (status, kind, expected) in [
        (
            StatusCode::UNAUTHORIZED,
            "authentication_error",
            error_codes::UPSTREAM_UNAUTHORIZED,
        ),
        (
            StatusCode::FORBIDDEN,
            "permission_denied",
            error_codes::UPSTREAM_FORBIDDEN,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            "insufficient_quota",
            error_codes::UPSTREAM_QUOTA_EXHAUSTED,
        ),
        (
            StatusCode::PAYMENT_REQUIRED,
            "insufficient_quota",
            error_codes::UPSTREAM_QUOTA_EXHAUSTED,
        ),
        (
            StatusCode::BAD_REQUEST,
            "content_policy_violation",
            error_codes::UPSTREAM_CONTENT_POLICY,
        ),
    ] {
        let body = serde_json::json!({"error":{
            "code":"upstream_capacity_exhausted","type":kind
        }});
        assert_eq!(
            classify_upstream_error_value(status, &body).category,
            expected,
            "{body}"
        );
    }
    for body in [
        serde_json::json!({"error":{"message":"upstream_capacity_exhausted"}}),
        serde_json::json!({"input":{"code":"upstream_capacity_exhausted"}}),
    ] {
        assert_eq!(
            classify_upstream_error_value(StatusCode::SERVICE_UNAVAILABLE, &body).category,
            error_codes::UPSTREAM_UNAVAILABLE
        );
    }
}
