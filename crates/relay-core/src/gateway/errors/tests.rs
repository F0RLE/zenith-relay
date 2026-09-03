use super::*;
use crate::ErrorOrigin;
use axum::body::{to_bytes, Body};
use std::time::Duration;

#[tokio::test]
async fn generated_errors_keep_the_original_diagnostic_category() {
    let response = api_error_with_origin_and_category(
        StatusCode::BAD_REQUEST,
        "upstream rejected the request",
        "invalid_request",
        "upstream_invalid_request",
        ErrorOrigin::Provider,
        Some("relay-request-1"),
    );

    assert_eq!(
        response
            .headers()
            .get("x-zenith-relay-error-origin")
            .and_then(|value| value.to_str().ok()),
        Some("provider")
    );
    assert_eq!(
        response
            .headers()
            .get("x-zenith-relay-error-category")
            .and_then(|value| value.to_str().ok()),
        Some("upstream_invalid_request")
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["code"], "invalid_request");
    assert_eq!(
        body["error"]["zenith_relay"]["category"],
        "upstream_invalid_request"
    );
    assert_eq!(body["error"]["zenith_relay"]["origin"], "provider");
}

#[tokio::test]
async fn adapter_failures_are_reported_as_relay_errors() {
    let response = api_error_with_origin_and_category(
        StatusCode::BAD_REQUEST,
        "upstream rejected the translated request",
        "invalid_request",
        "adapter_upstream_error",
        ErrorOrigin::Relay,
        Some("relay-request-bridge"),
    );

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["zenith_relay"]["origin"], "relay");
    assert_eq!(
        body["error"]["zenith_relay"]["category"],
        "adapter_upstream_error"
    );
}

#[tokio::test]
async fn native_provider_error_body_is_not_rewritten_for_diagnostics() {
    let original = br#"{"error":{"code":"bad_request","message":"upstream rejected request"}}"#;
    let response = super::super::response::proxy_error_response(
        StatusCode::BAD_REQUEST,
        &reqwest::header::HeaderMap::new(),
        Body::from(original.to_vec()),
        ErrorOrigin::Provider,
        "upstream_invalid_request",
        Some("relay-request-2"),
    );

    assert_eq!(
        response
            .headers()
            .get("x-zenith-relay-error-origin")
            .and_then(|value| value.to_str().ok()),
        Some("provider")
    );
    assert_eq!(
        response
            .headers()
            .get("x-zenith-relay-error-category")
            .and_then(|value| value.to_str().ok()),
        Some("upstream_invalid_request")
    );
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), original);
}

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
    assert!(recoverable_response_affinity_miss(
        StatusCode::CONFLICT,
        true,
        true,
        true,
    ));
}

#[test]
fn gateway_continuation_error_is_classified_as_relay_affinity_failure() {
    let payload = br#"{"error":{"code":"response_continuation_unavailable","message":"The Responses continuation route is unknown."}}"#;
    let classification = classify_upstream_error(StatusCode::CONFLICT, Some(payload));
    assert_eq!(classification.category, "response_affinity_miss");
    assert_eq!(
        classification.message,
        "Responses continuation route is unavailable"
    );
    assert_eq!(
        canonical_upstream_status(StatusCode::CONFLICT, classification.category),
        StatusCode::BAD_REQUEST
    );
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
fn zenith_gateway_invalid_request_is_detected_without_matching_generic_bad_requests() {
    assert!(zenith_gateway_invalid_request(
        br#"{"error":{"code":"invalid_request","message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}"#,
    ));
    assert!(zenith_gateway_invalid_request(
        br#"{"type":"error","response":{"error":{"message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}}"#,
    ));
    assert!(!zenith_gateway_invalid_request(
        br#"{"error":{"code":"invalid_request","message":"request payload is invalid"}}"#,
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
fn strict_responses_custom_tool_item_id_error_is_detected_without_matching_function_errors() {
    assert!(responses_custom_tool_item_id_requires_ctc_prefix(
        br#"{"error":{"message":"Invalid 'input[433].id': 'fc_abc'. Expected an ID that begins with 'ctc'."}}"#,
    ));
    assert!(!responses_custom_tool_item_id_requires_ctc_prefix(
        br#"{"error":{"message":"Invalid 'input[7].id': 'call_abc'. Expected an ID that begins with 'fc'."}}"#,
    ));
    assert!(!responses_custom_tool_item_id_requires_ctc_prefix(
        br#"{"error":{"message":"Expected an ID that begins with 'ctc'."}}"#,
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
                br#"{"error":{"code":"refresh_token_reused"}}"#.as_slice(),
                "upstream_refresh_token_reused",
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
                br#"{"error":{"code":"model_disabled","message":"Requested model is disabled"}}"#.as_slice(),
                "upstream_candidate_rejected",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"code":"vendor_route_42","message":"this route cannot serve the request"}}"#.as_slice(),
                "upstream_candidate_rejected",
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
fn deactivated_workspace_detection_requires_the_exact_structured_code() {
    for payload in [
        br#"{"detail":{"code":"deactivated_workspace"}}"#.as_slice(),
        br#"{"error":{"code":"deactivated_workspace"}}"#.as_slice(),
        br#"{"response":{"error":{"code":"deactivated_workspace"}}}"#.as_slice(),
    ] {
        assert!(is_deactivated_workspace(payload));
    }
    for payload in [
        br#"{"detail":{"code":"workspace_disabled"}}"#.as_slice(),
        br#"{"error":{"message":"deactivated_workspace"}}"#.as_slice(),
        br#"{"code":"deactivated_workspace"}"#.as_slice(),
        br#"not-json"#.as_slice(),
    ] {
        assert!(!is_deactivated_workspace(payload));
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
            br#"{"error":{"code":"service_unavailable","message":"request failed at https://gateway.example.invalid/v1; bearer secret"}}"#,
        )
        .is_none());
    assert!(preserved_upstream_error(
            &failure,
            br#"{"error":{"code":"service_unavailable","message":"quota exceeded for org-acme; contact admin@acme.test"}}"#,
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
        "upstream_model_unsupported",
        false
    ));
    assert!(retryable_failure(
        StatusCode::BAD_REQUEST,
        "upstream_candidate_rejected",
        false
    ));
    assert!(!retryable_failure(
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

#[test]
fn retry_after_supports_delta_seconds_and_http_dates() {
    let now = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(RETRY_AFTER, reqwest::header::HeaderValue::from_static("17"));
    assert_eq!(cooldown::retry_after_ms(&headers, now), Some(17_000));

    headers.insert(
        RETRY_AFTER,
        reqwest::header::HeaderValue::from_static("518400"),
    );
    assert_eq!(cooldown::retry_after_ms(&headers, now), Some(518_400_000));

    let date = httpdate::fmt_http_date(now + Duration::from_secs(23));
    headers.insert(RETRY_AFTER, date.parse().unwrap());
    assert_eq!(cooldown::retry_after_ms(&headers, now), Some(23_000));
}

#[test]
fn rate_limit_body_hint_uses_reset_time_and_marks_usage_limits_global() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"type":"usage_limit_reached","resets_at":1700000120,"resets_in_seconds":1}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(hint.retry_after_ms, Some(120_000));
    assert!(hint.global);
}

#[test]
fn rate_limit_body_hint_accepts_relative_reset_seconds() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"rate_limit","resets_in_seconds":"17"}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(hint.retry_after_ms, Some(17_000));
    assert!(!hint.global);
}

#[test]
fn rate_limit_body_hint_accepts_retry_after_and_message_delays() {
    let retry_after = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"rate_limit_exceeded","retry_after":"2.5"}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(retry_after.retry_after_ms, Some(2_500));

    let seconds = cooldown::rate_limit_body_hint_at(
            br#"{"response":{"error":{"code":"rate_limit_exceeded","message":"Please try again in 11.054s."}}}"#,
            UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        );
    assert_eq!(seconds.retry_after_ms, Some(11_054));

    let millis = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"message":"Please try again in 250ms."}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(millis.retry_after_ms, Some(250));
}

#[test]
fn rate_limit_body_hint_accepts_top_level_quota_variants() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"code":"rate_limit_reached","resets_in_seconds":9}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert_eq!(hint.retry_after_ms, Some(9_000));
    assert!(hint.global);
}

#[test]
fn quota_exhaustion_scope_is_global_without_a_global_body_hint() {
    assert_eq!(
        cooldown::rate_limit_scope("upstream_quota_exhausted", false, "gpt-5"),
        "*"
    );
    assert_eq!(
        cooldown::rate_limit_scope("upstream_rate_limited", false, "gpt-5"),
        "gpt-5"
    );
}

#[test]
fn websocket_connection_limit_is_account_global() {
    let hint = cooldown::rate_limit_body_hint_at(
        br#"{"error":{"code":"websocket_connection_limit_reached"}}"#,
        UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    );
    assert!(hint.global);
}

#[test]
fn rate_limit_delay_uses_the_stronger_hint_and_keeps_explicit_zero() {
    assert_eq!(
        cooldown::rate_limit_cooldown_ms(Some(1_000), Some(120_000), 1),
        120_000
    );
    assert_eq!(cooldown::rate_limit_cooldown_ms(Some(0), None, 5), 0);
}

#[test]
fn source_recovery_delay_overrides_automatic_but_not_provider_retry_after() {
    assert_eq!(
        cooldown::source_cooldown_ms(5_000, Some(60_000), false),
        60_000
    );
    assert_eq!(
        cooldown::source_cooldown_ms(120_000, Some(60_000), true),
        120_000
    );
    assert_eq!(cooldown::source_cooldown_ms(5_000, None, false), 5_000);
}

#[test]
fn no_header_rate_limit_backoff_is_exponential_and_capped() {
    assert_eq!(cooldown::exponential_backoff_ms(1), 1_000);
    assert_eq!(cooldown::exponential_backoff_ms(2), 2_000);
    assert_eq!(cooldown::exponential_backoff_ms(3), 4_000);
    assert_eq!(
        cooldown::exponential_backoff_ms(32),
        MAX_RATE_LIMIT_COOLDOWN_MS
    );
}

#[test]
fn failed_half_open_probes_back_off_without_shortening_retry_after() {
    assert_eq!(cooldown::half_open_backoff_ms(0, 2, true), 2_000);
    assert_eq!(cooldown::half_open_backoff_ms(60_000, 2, false), 60_000);
    assert_eq!(cooldown::half_open_backoff_ms(60_000, 2, true), 120_000);
    assert_eq!(cooldown::half_open_backoff_ms(60_000, 3, true), 240_000);
    assert_eq!(
        cooldown::half_open_backoff_ms(60_000, 32, true),
        MAX_RATE_LIMIT_COOLDOWN_MS
    );
    assert_eq!(
        cooldown::half_open_backoff_ms(MAX_RATE_LIMIT_RETRY_HINT_MS, 2, true),
        MAX_RATE_LIMIT_RETRY_HINT_MS
    );
}
