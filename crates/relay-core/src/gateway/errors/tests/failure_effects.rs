use super::*;

#[test]
fn http3_send_start_timeout_is_terminal_without_account_penalty_or_replay() {
    let error = AuthorizedRequestError::SendStartTimeout;
    assert_eq!(
        error.execution_certainty(),
        crate::scheduler::rotation::ExecutionCertainty::Unknown
    );
    let failure = AttemptFailure::authorized_request(error);
    assert_eq!(failure.status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(failure.category, error_codes::HTTP_SEND_START_TIMEOUT);
    assert!(!failure_category_requires_cooldown(failure.category));
    assert!(!failure_category_affects_account_state(failure.category));
    assert!(!retryable_failure(failure.status, failure.category, false));
    assert_eq!(
        crate::ErrorOrigin::Account.for_category(failure.category),
        crate::ErrorOrigin::Relay
    );
}

#[test]
fn local_http_headers_deadline_keeps_unknown_execution_without_account_penalty_or_replay() {
    let error = AuthorizedRequestError::HeadersTimeout;
    assert_eq!(
        error.execution_certainty(),
        crate::scheduler::rotation::ExecutionCertainty::Unknown
    );
    let failure = AttemptFailure::authorized_request(error);
    assert_eq!(failure.status, StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(failure.category, error_codes::HTTP_HEADERS_TIMEOUT);
    assert!(!failure_category_requires_cooldown(failure.category));
    assert!(!failure_category_affects_account_state(failure.category));
    assert!(!retryable_failure(failure.status, failure.category, false));
    assert_eq!(
        crate::ErrorOrigin::Account.for_category(failure.category),
        crate::ErrorOrigin::Relay
    );
}

#[test]
fn tpm_request_too_large_preserves_safe_rate_limit_failover() {
    let error = json!({
        "code": "rate_limit_exceeded",
        "type": "tokens",
        "message": "Request too large for synthetic-model on tokens per min (TPM): Limit 1000000, Requested 5000. The input or output tokens must be reduced in order to run successfully."
    });
    let mut bodies: Vec<Vec<u8>> = [
        json!({"error": error}),
        json!({"response": {"error": error}}),
        json!({"body": {"error": error}}),
    ]
    .iter()
    .map(|body| serde_json::to_vec(body).unwrap())
    .collect();
    bodies.push(error["message"].as_str().unwrap().as_bytes().to_vec());
    for body in bodies {
        let failure = AttemptFailure::status_with_body(StatusCode::TOO_MANY_REQUESTS, Some(&body));
        assert_eq!(failure.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(failure.category, error_codes::UPSTREAM_RATE_LIMITED);
        assert!(failure_category_requires_cooldown(failure.category));
        assert!(retryable_failure(failure.status, failure.category, false));
    }
    let frame = format!(
        "data: {}\n\n",
        json!({"type": "response.failed", "response": {"status": "failed", "error": error}})
    );
    let event = crate::gateway::streaming::parse_sse_event(frame.as_bytes());
    assert_eq!(
        event.error_category,
        Some(error_codes::UPSTREAM_RATE_LIMITED)
    );
    assert!(!event.semantic_output);

    let payload = br#"{"error":{"code":"request_too_large","message":"Request too large"}}"#;
    let failure = AttemptFailure::status_with_body(StatusCode::PAYLOAD_TOO_LARGE, Some(payload));
    assert_eq!(failure.status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(failure.category, error_codes::UPSTREAM_PAYLOAD_TOO_LARGE);
    assert!(!failure_category_requires_cooldown(failure.category));
    assert!(!retryable_failure(failure.status, failure.category, false));
}

#[test]
fn frozen_upstream_session_never_cools_an_account_or_rotates_the_request() {
    let error = json!({
        "type": "permission_error",
        "code": "session_blocked_by_cyber_policy",
        "message": "Synthetic conversation policy rejection"
    });
    for body in [
        json!({"error": error}),
        json!({"response": {"error": error}}),
    ] {
        let bytes = serde_json::to_vec(&body).unwrap();
        let failure = AttemptFailure::status_with_body(StatusCode::FORBIDDEN, Some(&bytes));
        assert_eq!(failure.category, error_codes::UPSTREAM_CONTENT_POLICY);
        assert!(!failure_category_requires_cooldown(failure.category));
        assert!(!failure_category_affects_account_state(failure.category));
        for opaque_predecessor in [false, true] {
            assert!(!retryable_failure(
                failure.status,
                failure.category,
                opaque_predecessor
            ));
        }
    }
    let frame = format!(
        "data: {}\n\n",
        json!({"type": "response.failed", "response": {"status": "failed", "error": error}})
    );
    let event = crate::gateway::streaming::parse_sse_event(frame.as_bytes());
    assert_eq!(
        event.error_category,
        Some(error_codes::UPSTREAM_CONTENT_POLICY)
    );
    assert!(!event.semantic_output);
}

#[test]
fn invalid_body_422_does_not_retry_or_penalize_accounts_even_with_server_error_type() {
    let body = br#"{"error":{"type":"server_error","message":"422: Invalid request body."}}"#;
    let failure = AttemptFailure::status_with_body(StatusCode::UNPROCESSABLE_ENTITY, Some(body));
    assert_eq!(failure.category, error_codes::UPSTREAM_INVALID_REQUEST);
    assert!(!failure_category_requires_cooldown(failure.category));
    assert!(!failure_category_affects_account_state(failure.category));
    for has_previous_response_id in [false, true] {
        assert!(!retryable_failure(
            failure.status,
            failure.category,
            has_previous_response_id
        ));
    }
}

#[test]
fn quota_rate_and_validation_failures_keep_distinct_effects() {
    for (code, category, cooldown) in [
        ("slow_down", "upstream_rate_limited", true),
        (
            "organization_spend_limit_exceeded",
            "upstream_quota_exhausted",
            true,
        ),
        (
            "project_spend_limit_exceeded",
            "upstream_quota_exhausted",
            true,
        ),
        ("INVALID_ARGUMENT", "upstream_invalid_request", false),
        ("validation_error", "upstream_invalid_request", false),
    ] {
        let classification = classify_upstream_error_value(
            StatusCode::BAD_GATEWAY,
            &json!({"error": {"code": code}}),
        );
        assert_eq!(classification.category, category, "{code}");
        assert_eq!(
            failure_category_requires_cooldown(category),
            cooldown,
            "{code}"
        );
    }
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
        "upstream_model_unsupported",
        false
    ));
    assert!(retryable_failure(
        StatusCode::BAD_REQUEST,
        "upstream_candidate_rejected",
        false
    ));
    assert!(retryable_failure(
        StatusCode::BAD_REQUEST,
        "upstream_candidate_rejected",
        true
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
    assert!(retryable_failure(
        StatusCode::UNAUTHORIZED,
        "upstream_unauthorized",
        false
    ));
    assert!(!retryable_failure(
        StatusCode::BAD_REQUEST,
        "upstream_context_too_large",
        false
    ));
    assert!(!retryable_failure(
        StatusCode::TOO_MANY_REQUESTS,
        error_codes::UPSTREAM_TURN_RATE_LIMITED,
        false
    ));
    assert!(!failure_category_requires_cooldown(
        error_codes::UPSTREAM_TURN_RATE_LIMITED
    ));
    assert!(!failure_category_affects_account_state(
        error_codes::UPSTREAM_TURN_RATE_LIMITED
    ));
    assert!(!retryable_failure(
        StatusCode::FORBIDDEN,
        "upstream_content_policy",
        false
    ));
    assert!(!failure_category_requires_cooldown(
        "upstream_invalid_request"
    ));
    for category in [
        "upstream_stream",
        "stream_incomplete",
        "stream_idle_timeout",
    ] {
        assert!(failure_category_requires_cooldown(category));
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

#[test]
fn retry_policy_requires_proven_rejection_and_preserves_connection_failover() {
    for (status, category, expected) in [
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            error_codes::UPSTREAM_SERVER_ERROR,
            false,
        ),
        (
            StatusCode::BAD_GATEWAY,
            error_codes::UPSTREAM_BAD_GATEWAY,
            false,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_UNAVAILABLE,
            false,
        ),
        (
            StatusCode::GATEWAY_TIMEOUT,
            error_codes::UPSTREAM_GATEWAY_TIMEOUT,
            false,
        ),
        (
            StatusCode::REQUEST_TIMEOUT,
            error_codes::UPSTREAM_REQUEST_TIMEOUT,
            false,
        ),
        (StatusCode::CONFLICT, error_codes::UPSTREAM_CONFLICT, false),
        (
            StatusCode::BAD_GATEWAY,
            error_codes::UPSTREAM_TRANSPORT_CONNECT,
            true,
        ),
        (
            StatusCode::BAD_GATEWAY,
            error_codes::UPSTREAM_TRANSPORT_TIMEOUT,
            false,
        ),
        (
            StatusCode::BAD_GATEWAY,
            error_codes::UPSTREAM_TRANSPORT_BODY,
            false,
        ),
        (
            StatusCode::BAD_GATEWAY,
            error_codes::UPSTREAM_TRANSPORT_REQUEST,
            false,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_OVERLOADED,
            true,
        ),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            error_codes::UPSTREAM_MODEL_CAPACITY,
            true,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            error_codes::UPSTREAM_RATE_LIMITED,
            true,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            error_codes::UPSTREAM_TURN_RATE_LIMITED,
            false,
        ),
    ] {
        assert_eq!(
            retryable_failure(status, category, false),
            expected,
            "{category}"
        );
    }
}

#[tokio::test]
async fn exhausted_quota_survives_the_cooldown_response_shape() {
    let failure = AttemptFailure::status_with_body(
        StatusCode::TOO_MANY_REQUESTS,
        Some(br#"{"error":{"type":"insufficient_quota"}}"#),
    );
    let response = cooldown_error(
        now_ms().saturating_add(60_000),
        Some(&failure),
        true,
        crate::ErrorOrigin::Relay,
    );
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
    let response = cooldown_error(
        now_ms().saturating_add(60_000),
        Some(&failure),
        false,
        crate::ErrorOrigin::Relay,
    );
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
    let response = cooldown_error(
        now_ms().saturating_add(60_000),
        Some(&failure),
        false,
        crate::ErrorOrigin::Relay,
    );
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
fn http_rejections_with_generated_usage_do_not_authorize_replay() {
    for payload in [
        serde_json::json!({"usage":{"output_tokens":7}}),
        serde_json::json!({"usage":{"output_tokens_details":{"reasoning_tokens":7}}}),
        serde_json::json!({"message":{"usage":{"output_tokens":7}}}),
        serde_json::json!({"usage":{"completion_tokens":7}}),
        serde_json::json!({"usageMetadata":{"thoughtsTokenCount":7}}),
    ] {
        for body in [
            serde_json::to_vec(&payload).unwrap(),
            format!("data: {payload}\n\n").into_bytes(),
        ] {
            let failure =
                AttemptFailure::status_with_body(StatusCode::TOO_MANY_REQUESTS, Some(&body));
            assert_eq!(
                failure.execution.certainty,
                crate::scheduler::rotation::ExecutionCertainty::Accepted
            );
        }
    }
    let failure = AttemptFailure::status_with_body(
        StatusCode::TOO_MANY_REQUESTS,
        Some(br#"{"usage":{"input_tokens":50,"output_tokens":0}}"#),
    );
    assert_eq!(
        failure.execution.certainty,
        crate::scheduler::rotation::ExecutionCertainty::NotSent
    );
}
