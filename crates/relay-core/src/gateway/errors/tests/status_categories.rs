use super::*;

#[test]
fn capacity_exhaustion_is_model_scoped_in_json_and_stream_terminals() {
    let body = serde_json::json!({"error":{
        "code":"model_capacity_exhausted","type":"api_error",
        "message":"Synthetic capacity refusal"
    }});
    let failure = AttemptFailure::status_with_body(
        StatusCode::BAD_GATEWAY,
        Some(&serde_json::to_vec(&body).unwrap()),
    );
    assert_eq!(failure.category, error_codes::UPSTREAM_MODEL_CAPACITY);
    assert_eq!(failure.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!failure_category_affects_account_state(failure.category));
    let frame = format!(
        "data: {}\n\n",
        serde_json::json!({
            "type":"response.failed","response":{
                "id":"resp_synthetic","status":"failed","error":body["error"]
            }
        })
    );
    let terminal = crate::gateway::streaming::parse_sse_event(frame.as_bytes());
    assert_eq!(
        terminal.error_category,
        Some(error_codes::UPSTREAM_MODEL_CAPACITY)
    );
    assert_eq!(terminal.error_status, Some(StatusCode::SERVICE_UNAVAILABLE));
    assert!(!terminal.semantic_output);
    let quota = AttemptFailure::status_with_body(
        StatusCode::TOO_MANY_REQUESTS,
        Some(br#"{"error":{"code":"insufficient_quota","message":"model_capacity_exhausted"}}"#),
    );
    assert_eq!(quota.category, error_codes::UPSTREAM_QUOTA_EXHAUSTED);
}

#[test]
fn siwc_usage_limit_without_an_error_code_is_a_pre_execution_quota_rejection() {
    let body = serde_json::json!({"error":{
        "type":"invalid_request_error",
        "message":"The ChatGPT user has reached their Subscription Sharing usage limit."
    }});
    let failure = AttemptFailure::status_with_body(
        StatusCode::TOO_MANY_REQUESTS,
        Some(&serde_json::to_vec(&body).unwrap()),
    );
    assert_eq!(failure.category, error_codes::UPSTREAM_QUOTA_EXHAUSTED);
    assert_eq!(
        failure.execution.certainty,
        crate::scheduler::rotation::ExecutionCertainty::NotSent
    );
    let frame = format!(
        "data: {}\n\n",
        serde_json::json!({
            "type":"response.failed","response":{"status":"failed","error":body["error"]}
        })
    );
    let terminal = crate::gateway::streaming::parse_sse_event(frame.as_bytes());
    assert_eq!(
        terminal.error_category,
        Some(error_codes::UPSTREAM_QUOTA_EXHAUSTED)
    );
    assert_eq!(terminal.error_status, Some(StatusCode::TOO_MANY_REQUESTS));
}

#[test]
fn normalized_rate_limit_stream_errors_keep_their_category() {
    let error = serde_json::json!({
        "code": error_codes::UPSTREAM_RATE_LIMITED,
        "message": "Basis Points stream could not be completed safely"
    });
    for body in [
        serde_json::json!({"type":"error","code":error["code"],"message":error["message"]}),
        serde_json::json!({"type":"response.failed","response":{"status":"failed","error":error}}),
    ] {
        assert_eq!(
            classify_upstream_error_value(StatusCode::BAD_GATEWAY, &body).category,
            error_codes::UPSTREAM_RATE_LIMITED
        );
        let frame = format!("data: {body}\n\n");
        let terminal = crate::gateway::streaming::parse_sse_event(frame.as_bytes());
        assert_eq!(
            terminal.error_category,
            Some(error_codes::UPSTREAM_RATE_LIMITED)
        );
        assert_eq!(terminal.error_status, Some(StatusCode::TOO_MANY_REQUESTS));
        assert_eq!(
            classify_upstream_error_value(StatusCode::UNAUTHORIZED, &body).category,
            error_codes::UPSTREAM_UNAUTHORIZED
        );
    }
}

#[test]
fn typed_adapter_stream_errors_keep_their_category_without_authorizing_replay() {
    for code in [
        error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
        error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
    ] {
        for body in [
            serde_json::json!({"error":{"type":"relay_adapter","code":code}}),
            serde_json::json!({"type":"error","code":code,"message":"Synthetic adapter failure","param":null,"sequence_number":0}),
        ] {
            let classified = classify_upstream_error_value(StatusCode::BAD_GATEWAY, &body);
            assert_eq!(classified.category, code);
            assert!(!failure_category_affects_account_state(code));
            assert!(!retryable_failure(StatusCode::BAD_GATEWAY, code, false));
            assert_eq!(
                AttemptFailure::classified_with_hint(
                    StatusCode::BAD_GATEWAY,
                    code,
                    Default::default()
                )
                .execution
                .certainty,
                crate::scheduler::rotation::ExecutionCertainty::Unknown
            );
            assert_eq!(
                classify_upstream_error_value(StatusCode::UNAUTHORIZED, &body).category,
                error_codes::UPSTREAM_UNAUTHORIZED
            );
        }
    }
    for ordinary in [
        serde_json::json!({"error":{"code":error_codes::ADAPTER_UPSTREAM_STREAM_INVALID}}),
        serde_json::json!({"type":"error","code":"synthetic_unknown"}),
        serde_json::json!({"type":"relay_adapter","code":error_codes::ADAPTER_UPSTREAM_STREAM_INVALID}),
    ] {
        assert_eq!(
            classify_upstream_error_value(StatusCode::BAD_GATEWAY, &ordinary).category,
            error_codes::UPSTREAM_BAD_GATEWAY
        );
    }
}

#[test]
fn basis_points_access_and_policy_refusals_are_not_quota_errors() {
    for (body, category) in [
        (br#"{"error":{"code":"basispoints_model_access_changed"}}"#.as_slice(), error_codes::UPSTREAM_MODEL_UNAVAILABLE),
        (br#"{"error":{"code":"model_not_allowed"}}"#.as_slice(), error_codes::UPSTREAM_MODEL_UNAVAILABLE),
        (b"403: Model access has changed".as_slice(), error_codes::UPSTREAM_MODEL_UNAVAILABLE),
        (br#"{"error":{"code":"insufficient_quota","message":"Model access has changed"}}"#.as_slice(), error_codes::UPSTREAM_MODEL_UNAVAILABLE),
        (b"403: This request was blocked by our usage policy.".as_slice(), error_codes::UPSTREAM_CONTENT_POLICY),
        (br#"{"error":{"type":"permission_error","code":"insufficient_quota","message":"This request was blocked by our usage policy."}}"#.as_slice(), error_codes::UPSTREAM_CONTENT_POLICY),
    ] {
        let failure = AttemptFailure::status_with_body(StatusCode::FORBIDDEN, Some(body));
        assert_eq!(failure.category, category);
        assert_eq!(failure.status, StatusCode::FORBIDDEN);
        assert!(!failure_category_affects_account_state(category));
        assert_eq!(retryable_failure(failure.status, category, false), category == error_codes::UPSTREAM_MODEL_UNAVAILABLE);
    }
}

#[test]
fn typed_stream_size_errors_keep_their_category_without_authorizing_replay() {
    let code = error_codes::STREAM_EVENT_TOO_LARGE;
    let error = serde_json::json!({"type":error_codes::STREAM_ERROR,"code":code});
    for body in [
        serde_json::json!({"error":error}),
        serde_json::json!({"type":"response.failed","response":{"error":error}}),
    ] {
        let failure = AttemptFailure::status_with_body(
            StatusCode::BAD_GATEWAY,
            Some(&serde_json::to_vec(&body).unwrap()),
        );
        assert_eq!(failure.category, code);
        assert_eq!(
            failure.execution.certainty,
            crate::scheduler::rotation::ExecutionCertainty::Unknown
        );
        assert!(!retryable_failure(failure.status, code, false));
        assert!(!failure_category_requires_cooldown(code));
        assert!(!failure_category_affects_account_state(code));
        assert_eq!(
            classify_upstream_error_value(StatusCode::UNAUTHORIZED, &body).category,
            error_codes::UPSTREAM_UNAUTHORIZED
        );
    }
    for error in [
        serde_json::json!({"code":code}),
        serde_json::json!({"type":error_codes::STREAM_ERROR,"code":"synthetic_unknown"}),
    ] {
        assert_eq!(
            classify_upstream_error_value(
                StatusCode::BAD_GATEWAY,
                &serde_json::json!({"error":error}),
            )
            .category,
            error_codes::UPSTREAM_BAD_GATEWAY
        );
    }
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
                br#"{"error":{"message":"The encrypted content for item rs_synthetic could not be verified. Reason: Encrypted content could not be decrypted or parsed."}}"#.as_slice(),
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
                StatusCode::UNPROCESSABLE_ENTITY,
                br#"{"error":{"type":"server_error","message":"422: Invalid request body."}}"#.as_slice(),
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
fn generic_gateway_rejection_does_not_cool_or_retry_the_candidate() {
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
    assert!(!retryable_failure(
        StatusCode::BAD_REQUEST,
        classification.category,
        false
    ));
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
