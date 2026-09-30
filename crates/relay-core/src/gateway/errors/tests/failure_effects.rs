use super::*;

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
