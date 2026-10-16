use super::*;

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
        body["error"]["message"],
        "Provider: upstream rejected the request"
    );
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
        body["error"]["message"],
        "Relay: upstream rejected the translated request"
    );
    assert_eq!(
        body["error"]["zenith_relay"]["category"],
        "adapter_upstream_error"
    );
}

#[test]
fn upstream_error_body_prefixes_the_selected_origin_once_and_keeps_diagnostics() {
    for (origin, expected) in [
        (ErrorOrigin::Account, "Account: invalid request"),
        (ErrorOrigin::Provider, "Provider: invalid request"),
        (ErrorOrigin::Relay, "Relay: invalid request"),
    ] {
        let original = br#"{"error":{"code":"bad_request","type":"validation_error","message":"Provider: invalid request"},"request_id":"synthetic-1"}"#;
        let body = prefix_error_body(original, origin);
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"]["message"], expected);
        assert_eq!(value["error"]["code"], "bad_request");
        assert_eq!(value["error"]["type"], "validation_error");
        assert_eq!(value["request_id"], "synthetic-1");
        assert_eq!(prefix_error_body(&body, origin), body);
    }
}

#[test]
fn scalar_upstream_error_text_is_prefixed_without_rewriting_error_codes() {
    let body = prefix_error_body(
        br#"{"error":"upstream connection closed"}"#,
        ErrorOrigin::Account,
    );
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"], "Account: upstream connection closed");

    let body = prefix_error_body(
        br#"{"error":"invalid_grant","error_description":"token is no longer valid"}"#,
        ErrorOrigin::Account,
    );
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"], "invalid_grant");
    assert_eq!(
        value["error_description"],
        "Account: token is no longer valid"
    );

    let body = prefix_error_body(
        br#"{"error":{"code":"invalid_grant"}}"#,
        ErrorOrigin::Account,
    );
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["code"], "invalid_grant");
    assert_eq!(value.get("message"), None);

    let body = prefix_error_body(
        br#"{"errors":[{"message":"Provider: request rejected"}]}"#,
        ErrorOrigin::Relay,
    );
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["errors"][0]["message"], "Relay: request rejected");

    assert_eq!(
        ErrorOrigin::Relay.prefix_message(" Account: Provider: request rejected"),
        "Relay: request rejected"
    );
}

#[tokio::test]
async fn proxy_error_response_prefixes_message_and_attaches_diagnostics() {
    let original = br#"{"error":{"code":"bad_request","message":"upstream rejected request"}}"#;
    let response = crate::gateway::response::proxy_error_response(
        StatusCode::BAD_REQUEST,
        &reqwest::header::HeaderMap::new(),
        original,
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
    let body: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(body["error"]["code"], "bad_request");
    assert_eq!(
        body["error"]["message"],
        "Provider: upstream rejected request"
    );
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
