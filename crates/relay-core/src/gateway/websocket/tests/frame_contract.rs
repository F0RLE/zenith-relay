use super::*;
use serde_json::json;

#[test]
fn websocket_model_guard_checks_text_and_binary_envelopes_only() {
    use crate::gateway::websocket::upstream::message_serves_rejected_model;
    use reqwest_websocket::Message;
    let payload = r#"{"type":"response.created","response":{"model":"gpt-5.6-luna"}}"#;
    for message in [
        Message::Text(payload.into()),
        Message::Binary(payload.as_bytes().to_vec().into()),
    ] {
        assert!(message_serves_rejected_model(&message, "gpt-6-astra"));
        assert!(!message_serves_rejected_model(&message, "gpt-5.6-luna"));
    }
    assert!(!message_serves_rejected_model(
        &Message::Text(r#"{"delta":"gpt-5.6-luna"}"#.into()),
        "gpt-6-astra"
    ));
}

#[test]
fn only_upstream_incomplete_failures_cool_the_candidate() {
    assert!(incomplete_requires_cooldown("upstream_websocket_closed"));
    assert!(incomplete_requires_cooldown("websocket_idle_timeout"));
    assert!(!incomplete_requires_cooldown("client_cancelled"));
    assert!(!incomplete_requires_cooldown("invalid_request"));
}

#[test]
fn response_incomplete_is_terminal_but_not_slot_success() {
    let terminal = event_terminal(&json!({
        "type": "response.incomplete",
        "response": {
            "id": "resp_incomplete",
            "status": "incomplete",
            "incomplete_details": {"reason": "max_output_tokens"}
        }
    }));

    assert_eq!(terminal.outcome, Some(EventTerminalOutcome::Incomplete));
    assert_eq!(terminal.error_category, Some("response_incomplete"));
    assert!(!incomplete_requires_cooldown("response_incomplete"));
}

#[test]
fn completed_websocket_event_does_not_override_an_explicit_failed_status() {
    let terminal = event_terminal(&json!({
        "type": "response.completed",
        "response": {"id": "resp_failed", "status": "failed"}
    }));
    assert_eq!(terminal.outcome, Some(EventTerminalOutcome::Failure));
    assert_eq!(
        terminal.error_category,
        Some(crate::error_codes::UPSTREAM_TERMINAL)
    );
}

#[test]
fn websocket_bootstrap_retries_only_empty_zero_token_incomplete() {
    let empty = vec![
        json!({"type": "response.created", "response": {"id": "resp_1"}}),
        json!({
            "type": "response.incomplete",
            "response": {"output": [], "usage": {"output_tokens": 0}}
        }),
    ];
    assert!(initial_payloads_are_empty_incomplete(&empty));

    let with_reasoning = vec![
        json!({"type": "response.reasoning_text.delta", "delta": "thinking"}),
        json!({
            "type": "response.incomplete",
            "response": {"output": [], "usage": {"output_tokens": 0}}
        }),
    ];
    assert!(!initial_payloads_are_empty_incomplete(&with_reasoning));

    let with_completed_item = vec![
        json!({"type": "response.output_item.done", "item": {"type": "message"}}),
        json!({
            "type": "response.incomplete",
            "response": {"output": [], "usage": {"output_tokens": 0}}
        }),
    ];
    assert!(!initial_payloads_are_empty_incomplete(&with_completed_item));

    let non_zero = vec![json!({
        "type": "response.incomplete",
        "response": {"output": [], "usage": {"output_tokens": 1}}
    })];
    assert!(!initial_payloads_are_empty_incomplete(&non_zero));
}

#[test]
fn websocket_setup_events_do_not_commit_route_ownership() {
    assert!(!semantic_output_payload(
        br#"{"type":"response.created","response":{"id":"resp_1"}}"#
    ));
    assert!(!semantic_output_payload(
        br#"{"type":"response.in_progress","response":{"id":"resp_1"}}"#
    ));
    assert!(semantic_output_payload(
        br#"{"type":"response.output_text.delta","delta":"hello"}"#
    ));
    assert!(!semantic_output_payload(
        br#"{"type":"response.compaction.delta","opaque":true}"#
    ));
    assert!(!semantic_output_payload(
        br#"{"type":"response.output_item.done","item":{"type":"compaction","encrypted_content":"opaque"}}"#
    ));
}

#[test]
fn websocket_unknown_or_malformed_frames_are_conservative() {
    assert!(semantic_output_payload(
        br#"{"type":"response.future_output_event"}"#
    ));
    assert!(semantic_output_payload(b"not-json"));
}

#[test]
fn http_fallback_preserves_opaque_compaction_data() {
    let terminal = super::super::parse_sse_event(
        b"event: response.compaction.delta\ndata: encrypted-compaction-fragment\n\n",
    );
    let message = fallback_event_message(&terminal, None, ErrorOrigin::Account)
        .ok()
        .flatten()
        .expect("opaque compaction must produce a WebSocket message");

    assert!(!message.semantic_output);
    match message.message {
        axum::extract::ws::Message::Text(text) => {
            assert_eq!(text.to_string(), "encrypted-compaction-fragment")
        }
        other => assert!(
            matches!(other, axum::extract::ws::Message::Text(_)),
            "UTF-8 compaction data should remain a text message"
        ),
    }
}

#[test]
fn named_http_fallback_scopes_json_compaction_but_rejects_unroutable_raw_data() {
    let json_terminal = super::super::parse_sse_event(
        br#"event: response.compaction.delta
data: {"type":"response.compaction.delta","delta":"opaque","stream_id":"other"}

"#,
    );
    let message = fallback_event_message(&json_terminal, Some("main"), ErrorOrigin::Account)
        .ok()
        .flatten()
        .expect("named compaction JSON must be forwarded");
    assert!(!message.semantic_output);
    let text = match message.message {
        axum::extract::ws::Message::Text(text) => Some(text),
        _ => None,
    }
    .expect("JSON compaction must be a text frame");
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["delta"], "opaque");
    assert_eq!(value["stream_id"], "main");

    let raw_terminal = super::super::parse_sse_event(
        b"event: response.compaction.delta\ndata: encrypted-compaction-fragment\n\n",
    );
    assert!(fallback_event_message(&raw_terminal, Some("main"), ErrorOrigin::Account).is_err());
}

#[test]
fn http_fallback_does_not_commit_response_setup_events() {
    let terminal = super::super::parse_sse_event(
        br#"data: {"type":"response.created","response":{"id":"resp_setup"}}

"#,
    );
    let message = fallback_event_message(&terminal, None, ErrorOrigin::Account)
        .ok()
        .flatten()
        .expect("setup event must still be forwarded");

    assert!(!message.semantic_output);
}

#[test]
fn http_fallback_prefixes_terminal_errors_for_the_selected_account() {
    let terminal = super::super::parse_sse_event(
        br#"event: response.failed
data: {"type":"response.failed","response":{"error":{"code":"server_error","message":"connection closed"}}}

"#,
    );

    for stream_id in [None, Some("main")] {
        let message = fallback_event_message(&terminal, stream_id, ErrorOrigin::Account)
            .ok()
            .flatten()
            .expect("terminal failure should be forwarded");
        let text = match message.message {
            axum::extract::ws::Message::Text(text) => text,
            other => panic!("expected text WebSocket frame, got {other:?}"),
        };
        let payload: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            payload["response"]["error"]["message"],
            "Account: connection closed"
        );
    }
}

#[test]
fn absolute_usage_reset_becomes_retry_delay() {
    let value = json!({
        "type": "error",
        "body": {"error": {"type": "usage_limit_reached", "resets_at": 1_700_000_120}}
    });
    assert_eq!(
        websocket_reset_delay_seconds(&value, 1_700_000_000),
        Some(120)
    );
}

#[test]
fn terminal_errors_never_keep_a_success_status() {
    assert_eq!(
        terminal_failure_status(Some(StatusCode::OK)),
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(
        terminal_failure_status(Some(StatusCode::TOO_MANY_REQUESTS)),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[test]
fn websocket_terminal_and_handshake_keep_original_failure_details() {
    let value = json!({"type": "response.failed", "response": {"error": {
        "code": "future_constraint", "type": "validation_error", "message": "Invalid field: temperature"
    }}});
    let terminal = event_terminal(&value);
    let details = terminal.upstream_error.unwrap();
    assert_eq!(details.code.as_deref(), Some("future_constraint"));
    assert_eq!(details.http_status, None);
    let failure = GatewayFailure::classified(
        StatusCode::BAD_REQUEST,
        "upstream_invalid_request",
        ErrorOrigin::Account,
    )
    .with_upstream_error(Some(details));
    let event = super::super::failure::gateway_error_event(&failure, None, None);
    assert_eq!(
        event["error"]["message"],
        "Account: Invalid field: temperature"
    );
    let handshake = GatewayFailure::upstream_status(
        StatusCode::UNPROCESSABLE_ENTITY,
        Some(br#"{"error":{"code":"validation_error","message":"Invalid field: temperature"}}"#),
        ErrorOrigin::Account,
    );
    assert_eq!(handshake.status, StatusCode::BAD_REQUEST);
    assert_eq!(handshake.upstream_error.unwrap().http_status, Some(422));
}

#[test]
fn upstream_status_accepts_string_status_codes() {
    let value = serde_json::json!({"type": "error", "status": "429"});
    assert_eq!(
        crate::gateway::errors::upstream_status_from_value(&value),
        Some(StatusCode::TOO_MANY_REQUESTS)
    );
}

#[test]
fn websocket_retry_headers_preserve_nested_reset_and_quota_hints() {
    let value = json!({
        "body": {
            "error": {"resets_in_seconds": 45},
            "headers": {"x-codex-primary-used-percent": "99"}
        }
    });
    let headers = websocket_retry_headers(&value);

    assert_eq!(
        headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok()),
        Some("45")
    );
    assert_eq!(
        headers
            .get("x-codex-primary-used-percent")
            .and_then(|value| value.to_str().ok()),
        Some("99")
    );
}

#[test]
fn websocket_cooldown_failure_keeps_retry_metadata() {
    let failure = GatewayFailure::cooldown(1_700_000_120_000);

    assert_eq!(failure.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(failure.category, "all_candidates_cooling_down");
    assert_eq!(failure.retry_at_ms, Some(1_700_000_120_000));
    assert_eq!(failure.origin, ErrorOrigin::Relay);
}

#[test]
fn websocket_errors_keep_the_source_origin_and_unmapped_category() {
    let failure = GatewayFailure::classified(
        StatusCode::BAD_REQUEST,
        "upstream_invalid_request",
        ErrorOrigin::Account,
    );
    let event =
        super::super::failure::gateway_error_event(&failure, Some("relay-request-3"), Some("main"));

    assert_eq!(event["stream_id"], "main");

    assert_eq!(event["error"]["code"], "invalid_request");
    assert!(event["error"]["message"]
        .as_str()
        .is_some_and(|message| message.starts_with("Account: ")));
    assert_eq!(
        event["error"]["zenith_relay"]["category"],
        "upstream_invalid_request"
    );
    assert_eq!(event["error"]["zenith_relay"]["origin"], "account");
    assert_eq!(
        event["error"]["zenith_relay"]["request_id"],
        "relay-request-3"
    );
}

#[test]
fn http_fallback_preserves_provider_and_account_error_origins() {
    let provider = Response::builder()
        .status(StatusCode::TOO_MANY_REQUESTS)
        .header(RELAY_ERROR_ORIGIN_HEADER, "provider")
        .body(Body::empty())
        .unwrap();
    assert_eq!(fallback_response_origin(&provider), ErrorOrigin::Provider);

    let account = Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(RELAY_ERROR_ORIGIN_HEADER, "account")
        .body(Body::empty())
        .unwrap();
    assert_eq!(fallback_response_origin(&account), ErrorOrigin::Account);
}

#[test]
fn http_fallback_rejects_unknown_or_untrusted_error_origins() {
    let unknown = Response::builder()
        .status(StatusCode::BAD_GATEWAY)
        .header(RELAY_ERROR_ORIGIN_HEADER, "external")
        .body(Body::empty())
        .unwrap();
    assert_eq!(fallback_response_origin(&unknown), ErrorOrigin::Relay);

    let upstream = Response::builder()
        .status(StatusCode::OK)
        .header(RELAY_UPSTREAM_ORIGIN_HEADER, "provider")
        .body(Body::empty())
        .unwrap();
    assert_eq!(fallback_response_origin(&upstream), ErrorOrigin::Provider);
}
