use super::*;

#[test]
fn invalid_workspace_is_terminal_only_for_explicit_forbidden_code() {
    for body in [
        serde_json::json!({"detail":{"code":"invalid_workspace_selected"}}),
        serde_json::json!({"error":{"code":"invalid_workspace_selected"}}),
    ] {
        let bytes = serde_json::to_vec(&body).unwrap();
        let failure = AttemptFailure::status_with_body(StatusCode::FORBIDDEN, Some(&bytes));
        assert_eq!(failure.category, error_codes::UPSTREAM_INVALID_WORKSPACE);
        assert_eq!(failure.status, StatusCode::FORBIDDEN);
        assert!(!retryable_failure(failure.status, failure.category, false));
        assert!(!retryable_failure(failure.status, failure.category, true));
        assert!(!failure_category_requires_cooldown(failure.category));
        assert!(!failure_category_affects_account_state(failure.category));
        for status in [StatusCode::UNAUTHORIZED, StatusCode::BAD_GATEWAY] {
            assert_ne!(
                classify_upstream_error_value(status, &body).category,
                error_codes::UPSTREAM_INVALID_WORKSPACE
            );
        }
    }
}

#[test]
fn workspace_prose_nested_hints_and_conflicting_codes_are_not_terminal_evidence() {
    for body in [
        r#"invalid_workspace_selected"#,
        r#"{"error":{"message":"invalid_workspace_selected"}}"#,
        r#"{"input":{"error":{"code":"invalid_workspace_selected"}}}"#,
        r#"{"detail":{"code":"other"},"error":{"code":"invalid_workspace_selected"}}"#,
        r#"{"error":{"code":"other","code":"invalid_workspace_selected"}}"#,
        r#"{"error":{"code":"invalid_workspace_selected"},"error":{"code":"other"}}"#,
        "{\"error\":{\"code\":\"invalid_workspace_selected\"}}\n\ndata: {}\n\n",
    ] {
        assert_ne!(
            classify_upstream_error(StatusCode::FORBIDDEN, Some(body.as_bytes())).category,
            error_codes::UPSTREAM_INVALID_WORKSPACE,
            "{body}"
        );
    }
}

#[test]
fn workspace_header_is_bounded_strict_and_subordinate_to_the_body_code() {
    use base64::Engine;
    let encode = |body: &str| base64::engine::general_purpose::STANDARD.encode(body);
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        "x-error-json",
        encode(r#"{"error":{"code":"invalid_workspace_selected"}}"#)
            .parse()
            .unwrap(),
    );
    for body in [
        None,
        Some(br#"{"error":{"message":"Synthetic refusal"}}"#.as_slice()),
    ] {
        let failure = AttemptFailure::status_with_headers(StatusCode::FORBIDDEN, body, &headers);
        assert_eq!(failure.category, error_codes::UPSTREAM_INVALID_WORKSPACE);
        assert!(!retryable_failure(failure.status, failure.category, false));
        assert!(!failure_category_affects_account_state(failure.category));
    }
    for body in [
        br#"{"detail":{"code":"other"}}"#.as_slice(),
        br#"{"error":{"code":"other"}}"#,
        br#"{"error":{"code":"other","code":"invalid_workspace_selected"}}"#,
        b"malformed",
    ] {
        assert_ne!(
            classify_upstream_error_with_headers(StatusCode::FORBIDDEN, Some(body), &headers)
                .category,
            error_codes::UPSTREAM_INVALID_WORKSPACE
        );
    }
    assert_ne!(
        classify_upstream_error_with_headers(StatusCode::UNAUTHORIZED, None, &headers).category,
        error_codes::UPSTREAM_INVALID_WORKSPACE
    );
    for value in [
        "invalid base64".into(),
        encode(r#"{"error":{"message":"invalid_workspace_selected"}}"#),
        encode(r#"{"error":{"code":"other","code":"invalid_workspace_selected"}}"#),
        encode(&format!(
            r#"{{"error":{{"code":"invalid_workspace_selected"}},"padding":"{}"}}"#,
            "x".repeat(8192)
        )),
    ] {
        headers.insert("x-error-json", value.parse().unwrap());
        assert_ne!(
            classify_upstream_error_with_headers(StatusCode::FORBIDDEN, None, &headers).category,
            error_codes::UPSTREAM_INVALID_WORKSPACE
        );
    }
    headers.insert(
        "x-error-json",
        encode(r#"{"error":{"code":"invalid_workspace_selected"}}"#)
            .parse()
            .unwrap(),
    );
    headers.append("x-error-json", "ambiguous".parse().unwrap());
    assert_ne!(
        classify_upstream_error_with_headers(StatusCode::FORBIDDEN, None, &headers).category,
        error_codes::UPSTREAM_INVALID_WORKSPACE
    );
}
