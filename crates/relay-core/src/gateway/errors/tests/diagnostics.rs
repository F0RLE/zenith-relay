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
    let response = crate::gateway::response::proxy_error_response(
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
