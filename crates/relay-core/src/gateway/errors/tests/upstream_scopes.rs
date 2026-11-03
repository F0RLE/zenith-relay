use super::*;
use serde_json::Value;

#[test]
fn cloudflare_blocks_keep_credentials_valid_but_retain_route_backoff() {
    for body in [
        json!({"cloudflare_error":true,"code":1010}),
        json!({"error":{"cloudflare_error":true,"error_code":"1010"}}),
        json!({"error":{"error_name":"browser_signature_banned"}}),
        json!({"cloudflare_error":true,"errors":[{"code":1010}]}),
    ] {
        let bytes = serde_json::to_vec(&body).unwrap();
        let failure = AttemptFailure::status_with_body(StatusCode::FORBIDDEN, Some(&bytes));
        assert_eq!(
            failure.category,
            error_codes::UPSTREAM_EDGE_CHALLENGE,
            "{body}"
        );
        assert!(!failure_category_affects_account_state(failure.category));
        assert!(failure_category_requires_cooldown(failure.category));
    }
    let html = b"<html><h1>Error code: 1010</h1><p>Synthetic edge rejection</p></html>";
    assert_eq!(
        classify_upstream_error(StatusCode::FORBIDDEN, Some(html)).category,
        error_codes::UPSTREAM_EDGE_CHALLENGE
    );
}

#[test]
fn unmarked_codes_and_echoed_input_do_not_override_genuine_auth_refusals() {
    let malformed = b"{\"input\":\"error code: 1010\"";
    assert_ne!(
        classify_upstream_error(StatusCode::FORBIDDEN, Some(malformed)).category,
        error_codes::UPSTREAM_EDGE_CHALLENGE
    );
    for body in [
        json!({"error":{"code":1010}}),
        json!({"error":{"type":"permission_error"},"input":{"cloudflare_error":true,"code":1010}}),
    ] {
        assert_ne!(
            classify_upstream_error_value(StatusCode::FORBIDDEN, &body).category,
            error_codes::UPSTREAM_EDGE_CHALLENGE
        );
    }
    let body = json!({"error":{"code":"account_deactivated"},"input":{"error_name":"browser_signature_banned"}});
    assert_eq!(
        classify_upstream_error_value(StatusCode::FORBIDDEN, &body).category,
        error_codes::UPSTREAM_ACCOUNT_DISABLED
    );
    let body =
        json!({"error":{"code":"account_deactivated","error_name":"browser_signature_banned"}});
    assert_eq!(
        classify_upstream_error_value(StatusCode::FORBIDDEN, &body).category,
        error_codes::UPSTREAM_ACCOUNT_DISABLED
    );
    let body = json!({"cloudflare_error":true,"code":1010});
    assert_ne!(
        classify_upstream_error_value(StatusCode::UNAUTHORIZED, &body).category,
        error_codes::UPSTREAM_EDGE_CHALLENGE
    );
}

fn handshake_error() -> Value {
    json!({"error":{"type":"upstream_error","code":"upstream_error",
        "message":"failed to WebSocket dial: expected handshake response status code 101 but got 403"}})
}

#[test]
fn explicit_internal_handshake_rejection_is_terminal_not_an_account_failure() {
    let body = serde_json::to_vec(&handshake_error()).unwrap();
    let failure = AttemptFailure::status_with_body(StatusCode::FORBIDDEN, Some(&body));
    assert_eq!(
        failure.category,
        error_codes::UPSTREAM_WEBSOCKET_HANDSHAKE_REJECTED
    );
    assert!(!failure_category_affects_account_state(failure.category));
    assert!(!failure_category_requires_cooldown(failure.category));
    for bound in [false, true] {
        assert!(!retryable_failure(failure.status, failure.category, bound));
    }
    for (field, value) in [
        ("type", "permission_error"),
        ("code", "account_deactivated"),
        ("message", "Other upstream 403"),
    ] {
        let mut body = handshake_error();
        body["error"][field] = json!(value);
        assert_ne!(
            classify_upstream_error_value(StatusCode::FORBIDDEN, &body).category,
            error_codes::UPSTREAM_WEBSOCKET_HANDSHAKE_REJECTED
        );
    }
    assert_ne!(
        classify_upstream_error(StatusCode::UNAUTHORIZED, Some(&body)).category,
        error_codes::UPSTREAM_WEBSOCKET_HANDSHAKE_REJECTED
    );
}

#[test]
fn first_json_error_owns_a_forbidden_response_with_a_later_sse_event() {
    for (body, expected) in [
        (
            handshake_error(),
            error_codes::UPSTREAM_WEBSOCKET_HANDSHAKE_REJECTED,
        ),
        (
            json!({"error":{"code":"content_policy_violation"}}),
            error_codes::UPSTREAM_CONTENT_POLICY,
        ),
        (
            json!({"error":{"code":"account_deactivated"}}),
            error_codes::UPSTREAM_ACCOUNT_DISABLED,
        ),
    ] {
        let response = format!(
            "{body}\n\nevent: response.failed\ndata: {{\"error\":{{\"code\":\"invalid_api_key\"}}}}\n\n"
        );
        assert_eq!(
            classify_upstream_error(StatusCode::FORBIDDEN, Some(response.as_bytes())).category,
            expected
        );
    }
    let response = format!("{} invalid trailing JSON", handshake_error());
    assert_ne!(
        classify_upstream_error(StatusCode::FORBIDDEN, Some(response.as_bytes())).category,
        error_codes::UPSTREAM_WEBSOCKET_HANDSHAKE_REJECTED
    );
    let duplicate = b"{\"error\":{\"code\":\"account_deactivated\",\"code\":\"content_policy_violation\"}}\n\ndata: {}\n\n";
    assert_eq!(
        classify_upstream_error(StatusCode::FORBIDDEN, Some(duplicate)).category,
        error_codes::UPSTREAM_ACCOUNT_DISABLED
    );
}
