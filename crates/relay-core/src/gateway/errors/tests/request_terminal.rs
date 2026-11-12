use super::*;

fn sse_terminal(event: &Value) -> crate::gateway::streaming::TerminalEvent {
    let frame = format!("event: error\ndata: {event}\n\n");
    crate::gateway::streaming::parse_sse_event(frame.as_bytes())
}

#[test]
fn context_errors_preserve_explicit_auth_throttle_and_server_statuses() {
    for body in [
        r#"{"error":{"code":"context_length_exceeded"}}"#,
        r#"{"error":{"message":"Maximum context length exceeded"}}"#,
        "Maximum context length exceeded",
    ] {
        let client_error =
            AttemptFailure::status_with_body(StatusCode::BAD_REQUEST, Some(body.as_bytes()));
        assert_eq!(
            client_error.category,
            error_codes::UPSTREAM_CONTEXT_TOO_LARGE
        );
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::TOO_MANY_REQUESTS,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
            StatusCode::SERVICE_UNAVAILABLE,
            StatusCode::GATEWAY_TIMEOUT,
        ] {
            let failure = AttemptFailure::status_with_body(status, Some(body.as_bytes()));
            assert_ne!(
                failure.category,
                error_codes::UPSTREAM_CONTEXT_TOO_LARGE,
                "{status}: {body}"
            );
            assert_eq!(failure.status, status, "{body}");
        }
    }
}

#[test]
fn streaming_context_errors_distinguish_omitted_status_from_explicit_server_errors() {
    let mut legacy = json!({"error":{"message":"Maximum context length exceeded"}});
    assert_eq!(
        upstream_event_failure_category(Some("error"), &legacy),
        Some(error_codes::UPSTREAM_CONTEXT_TOO_LARGE)
    );
    legacy["status"] = json!(503);
    assert_eq!(
        upstream_event_failure_category(Some("error"), &legacy),
        Some(error_codes::UPSTREAM_UNAVAILABLE)
    );
    for code in [
        "context_length_exceeded",
        "context_window_exceeded",
        "context_too_large",
    ] {
        for mut event in [
            json!({"error":{"code":code}}),
            json!({"response":{"status":"failed","error":{"code":code}}}),
        ] {
            assert_eq!(
                upstream_event_failure_category(Some("error"), &event),
                Some(error_codes::UPSTREAM_CONTEXT_TOO_LARGE)
            );
            event["status"] = json!(400);
            assert_eq!(
                upstream_event_failure_category(Some("error"), &event),
                Some(error_codes::UPSTREAM_CONTEXT_TOO_LARGE)
            );
            event["status"] = json!(503);
            assert_eq!(
                upstream_event_failure_category(Some("error"), &event),
                Some(error_codes::UPSTREAM_UNAVAILABLE)
            );
            let parsed = sse_terminal(&event);
            assert_eq!(
                parsed.error_category,
                Some(error_codes::UPSTREAM_UNAVAILABLE)
            );
            assert_eq!(parsed.error_status, Some(StatusCode::SERVICE_UNAVAILABLE));
            event["status"] = json!(401);
            assert_eq!(
                upstream_event_failure_category(Some("error"), &event),
                Some(error_codes::UPSTREAM_UNAUTHORIZED)
            );
        }
    }
}

#[test]
fn access_program_refusals_are_terminal_without_account_cooldown_or_retry() {
    for (code, category, status) in [
        (
            "invalid_access_program",
            error_codes::UPSTREAM_ACCESS_PROGRAM_INVALID,
            StatusCode::BAD_REQUEST,
        ),
        (
            "unsupported_access_program",
            error_codes::UPSTREAM_ACCESS_PROGRAM_INVALID,
            StatusCode::BAD_REQUEST,
        ),
        (
            "access_program_not_enabled",
            error_codes::UPSTREAM_ACCESS_PROGRAM_NOT_ENABLED,
            StatusCode::FORBIDDEN,
        ),
    ] {
        for body in [
            json!({"code":code}),
            json!({"error":{"code":code}}),
            json!({"detail":{"code":code}}),
            json!({"response":{"status":"failed","error":{"code":code}}}),
            json!({"body":{"error":{"code":code}}}),
        ] {
            let bytes = serde_json::to_vec(&body).unwrap();
            let failure = AttemptFailure::status_with_body(status, Some(&bytes));
            assert_eq!(failure.category, category);
            assert_eq!(failure.status, status);
            assert!(failure_category_is_request_terminal(category));
            assert!(!failure_category_requires_cooldown(category));
            assert!(!failure_category_affects_account_state(category));
            for continuation in [false, true] {
                assert!(!retryable_failure(status, category, continuation));
            }
            assert_eq!(
                upstream_event_failure_category(Some("error"), &body),
                Some(category)
            );
            let mut stream_event = body.clone();
            stream_event["type"] = json!("error");
            let parsed = sse_terminal(&stream_event);
            assert_eq!(parsed.error_category, Some(category));
            assert_eq!(parsed.error_status, Some(status));
            for wrong_status in [
                StatusCode::UNAUTHORIZED,
                StatusCode::TOO_MANY_REQUESTS,
                StatusCode::SERVICE_UNAVAILABLE,
            ] {
                let failure = AttemptFailure::status_with_body(wrong_status, Some(&bytes));
                assert_ne!(failure.category, category);
                assert_eq!(failure.status, wrong_status);
                let mut event = body.clone();
                event["status"] = json!(wrong_status.as_u16());
                assert_ne!(
                    upstream_event_failure_category(Some("error"), &event),
                    Some(category)
                );
            }
        }
    }
}

#[test]
fn access_program_proof_does_not_borrow_from_prose_or_conflicting_discriminators() {
    for body in [
        json!({"error":{"message":"invalid_access_program"}}),
        json!({"error":{"type":"invalid_access_program"}}),
        json!({"input":{"error":{"code":"invalid_access_program"}}}),
        json!({"code":"other","error":{"code":"invalid_access_program"}}),
        json!({"error":{"code":null,"type":"invalid_access_program"}}),
        json!({"error":{"code":"other"},"response":{"error":{"code":"invalid_access_program"}}}),
    ] {
        assert_ne!(
            classify_upstream_error_value(StatusCode::BAD_REQUEST, &body).category,
            error_codes::UPSTREAM_ACCESS_PROGRAM_INVALID
        );
        assert_ne!(
            upstream_event_failure_category(Some("error"), &body),
            Some(error_codes::UPSTREAM_ACCESS_PROGRAM_INVALID)
        );
    }
    for body in [
        br#"{"error":{"code":"other","code":"invalid_access_program"}}"#.as_slice(),
        br#"{"error":{"code":"other","co\u0064e":"invalid_access_program"}}"#,
        br#"{"error":{"code":"invalid_access_program"},"extra":{"x":1,"x":2}}"#,
    ] {
        assert_ne!(
            classify_upstream_error(StatusCode::BAD_REQUEST, Some(body)).category,
            error_codes::UPSTREAM_ACCESS_PROGRAM_INVALID
        );
    }
}
