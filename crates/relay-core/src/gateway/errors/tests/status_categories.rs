use super::*;

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
                StatusCode::NOT_FOUND,
                br#"{"error":{"code":"model_not_found","message":"The model `gpt-6-astra-degrade2-luna-1p-codexswic-ev3` does not exist or you do not have access to it."}}"#.as_slice(),
                "upstream_route_degraded",
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
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"error":{"type":"invalid_request_error","code":"model_disabled"}}"#.as_slice(),
                "upstream_candidate_rejected",
            ),
            (
                StatusCode::BAD_REQUEST,
                br#"{"error":{"status":"INVALID_ARGUMENT"}}"#.as_slice(),
                "upstream_invalid_request",
            ),
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"error":{"code":"validation_error"}}"#.as_slice(),
                "upstream_invalid_request",
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
fn generic_gateway_rejection_remains_a_candidate_failure() {
    let value: Value = serde_json::from_slice(
            br#"{"type":"error","error":{"type":"invalid_request_error","code":"invalid_request","message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}"#,
        )
        .unwrap();

    let classification = classify_upstream_error_value(StatusCode::BAD_GATEWAY, &value);
    assert_eq!(classification.category, "upstream_candidate_rejected");
    assert_eq!(
        upstream_event_failure_category(Some("error"), &value),
        Some("upstream_candidate_rejected")
    );
    assert!(failure_category_requires_cooldown(classification.category));
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

    let redacted = preserved_upstream_error(
            &failure,
            br#"{"error":{"code":"service_unavailable","message":"request failed at https://gateway.example.invalid/v1; bearer secret"}}"#,
        )
        .unwrap();
    assert!(!redacted.message.contains("https://"));
    assert!(!redacted.message.contains("secret"));
    let redacted = preserved_upstream_error(
            &failure,
            br#"{"error":{"code":"service_unavailable","message":"quota exceeded for org-acme; contact admin@acme.test"}}"#,
        )
        .unwrap();
    assert!(!redacted.message.contains("org-acme"));
    assert!(!redacted.message.contains("admin@"));
    let unknown = preserved_upstream_error(
        &failure,
        br#"{"error":{"code":"provider_error","message":"upstream diagnostic"}}"#,
    )
    .unwrap();
    assert_eq!(unknown.code, "provider_error");
    assert_eq!(unknown.message, "upstream diagnostic");
}
