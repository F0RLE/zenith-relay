use super::*;
use crate::gateway::test_support::test_usage_event;
use axum::body::Body;
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderValue;

#[test]
fn buffered_json_and_account_stream_keep_original_failure_details() {
    for (body, account_stream, expected_code, expected_message) in [
        (
            br#"{"status":"failed","error":{"code":"future_constraint","message":"Constraint check failed"},"input":"synthetic-private"}"#.as_slice(),
            false,
            "future_constraint",
            "Constraint check failed",
        ),
        (
            br#"{"status":"cancelled","error":{"code":"client_cancelled","message":"Request was cancelled"}}"#.as_slice(),
            false,
            "client_cancelled",
            "Request was cancelled",
        ),
        (
            b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"future_constraint\",\"message\":\"Constraint check failed\"}}}\n\n".as_slice(),
            true,
            "future_constraint",
            "Constraint check failed",
        ),
    ] {
        let failure = completed_upstream_response(body, account_stream, None).unwrap_err();
        let details = failure.upstream_error.unwrap();
        assert_eq!(details.code.as_deref(), Some(expected_code));
        assert_eq!(details.message.as_deref(), Some(expected_message));
        if expected_code == "future_constraint" {
            assert!(!serde_json::to_string(&details).unwrap().contains("synthetic-private"));
            assert_eq!(failure.preserved.unwrap().message, expected_message);
        }
    }
}

#[test]
fn non_stream_usage_normalizes_cached_reasoning_and_total_tokens() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"response":{"response":{"service_tier":"priority","usage":{"input_tokens":16,"input_tokens_details":{"cached_tokens":30},"output_tokens":5,"output_tokens_details":{"reasoning_tokens":30},"total_tokens":10}}}}"#,
    );

    assert_eq!(event.input_tokens, Some(16));
    assert_eq!(event.cached_input_tokens, Some(16));
    assert_eq!(event.reasoning_tokens, Some(5));
    assert_eq!(event.output_tokens, Some(5));
    assert_eq!(event.total_tokens, Some(21));
    assert_eq!(event.applied_service_tier, Some("priority".to_string()));
}

#[test]
fn response_service_tier_preserves_upstream_text_and_prefers_nested_response() {
    assert_eq!(
        response_service_tier(&serde_json::json!({"service_tier": "flex"})),
        Some("flex".to_string())
    );
    assert_eq!(
        response_service_tier(
            &serde_json::json!({"service_tier": "default", "response": {"service_tier": "ultrafast"}})
        ),
        Some("ultrafast".to_string())
    );
    assert_eq!(response_service_tier(&serde_json::json!({})), None);
    assert_eq!(
        response_service_tier(&serde_json::json!({"service_tier": "not a tier"})),
        None
    );
}

#[test]
fn anthropic_usage_adds_cache_read_and_creation_to_total_input() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_read_input_tokens":40,"cache_creation_input_tokens":20,"output_tokens":10}}"#,
    );

    assert_eq!(event.input_tokens, Some(160));
    assert_eq!(event.cached_input_tokens, Some(40));
    assert_eq!(event.cache_write_input_tokens, Some(20));
    assert_eq!(event.output_tokens, Some(10));
    assert_eq!(event.total_tokens, Some(170));
}

#[test]
fn anthropic_cache_creation_reports_actual_write_lifetime() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_1h_input_tokens":20},"output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_ttl.as_deref(), Some("1h"));

    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_5m_input_tokens":20},"output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_ttl.as_deref(), Some("5m"));
}

#[test]
fn cache_usage_keeps_all_reported_provider_windows() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_creation":{"ephemeral_1h_input_tokens":10,"ephemeral_5m_input_tokens":10,"ephemeral_15m_input_tokens":0},"output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_ttl.as_deref(), Some("5m, 1h"));

    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"cache_write_ttl":"15m","output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_ttl.as_deref(), Some("15m"));
}

#[test]
fn cache_usage_accepts_generic_provider_window_fields() {
    let usage = serde_json::json!({
        "cache_creation_ttl": "2h",
        "cache_creation_tokens_5m": 2,
        "input_tokens_details": {
            "cache_write_tokens_30m": 5
        },
        "prompt_cache_ttl": "24h",
        "cache_creation_input_tokens": 7
    });

    assert_eq!(
        cache_write_ttl_from_usage(&usage).as_deref(),
        Some("5m, 30m, 2h")
    );
}

#[test]
fn cache_usage_keeps_unknown_windows_unreported() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_creation_input_tokens":20,"output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_input_tokens, Some(20));
    assert_eq!(event.cache_write_ttl, None);
}

#[test]
fn cache_usage_keeps_provider_reported_ttl_without_write_counter() {
    let mut event = test_usage_event();
    populate_tokens(
        &mut event,
        br#"{"usage":{"input_tokens":100,"cache_write_ttl":"45m","output_tokens":10}}"#,
    );
    assert_eq!(event.cache_write_input_tokens, None);
    assert_eq!(event.cache_write_ttl.as_deref(), Some("45m"));
}

#[test]
fn proxy_keeps_safe_native_retry_and_request_headers_only() {
    let mut upstream = reqwest::header::HeaderMap::new();
    upstream.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    upstream.insert("retry-after", HeaderValue::from_static("12"));
    upstream.insert("request-id", HeaderValue::from_static("req_native"));
    upstream.insert(
        "x-codex-turn-state",
        HeaderValue::from_static("account-scoped-state"),
    );
    upstream.insert(
        "anthropic-ratelimit-requests-reset",
        HeaderValue::from_static("2026-08-02T00:00:00Z"),
    );
    upstream.insert("authorization", HeaderValue::from_static("Bearer secret"));
    upstream.insert("set-cookie", HeaderValue::from_static("session=secret"));
    upstream.insert("server", HeaderValue::from_static("provider-internal"));

    let response = proxy_response(
        reqwest::StatusCode::TOO_MANY_REQUESTS,
        &upstream,
        Body::empty(),
    );
    assert_eq!(response.headers()[CONTENT_TYPE], "application/json");
    assert_eq!(response.headers()["retry-after"], "12");
    assert_eq!(response.headers()["request-id"], "req_native");
    assert!(response.headers().get("x-codex-turn-state").is_none());
    assert_eq!(
        response.headers()["anthropic-ratelimit-requests-reset"],
        "2026-08-02T00:00:00Z"
    );
    assert!(response.headers().get("authorization").is_none());
    assert!(response.headers().get("set-cookie").is_none());
    assert!(response.headers().get("server").is_none());
}

#[test]
fn buffered_success_with_a_degraded_model_stays_retryable_only_while_blocking() {
    let body = br#"{"id":"resp_test","model":"gpt-6-astra-degrade2","output":[]}"#;
    let failure = completed_upstream_response(body, false, Some("gpt-6-astra")).unwrap_err();
    assert_eq!(failure.failure.category, "upstream_route_degraded");
    assert_eq!(
        failure.execution.certainty,
        crate::scheduler::rotation::ExecutionCertainty::NotSent
    );
    assert!(completed_upstream_response(body, false, None).is_ok());
}

#[test]
fn buffered_model_mismatch_is_rejected_but_generated_output_is_not_replayed() {
    for body in [
        br#"{"model":"gpt-5.6-luna","output":[{"type":"message","content":[{"type":"output_text","text":"synthetic"}]}]}"#.as_slice(),
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-luna\",\"output\":[]}}\n\n".as_slice(),
    ] {
        let failure = completed_upstream_response(body, true, Some("gpt-6-astra")).unwrap_err();
        assert_eq!(failure.failure.category, "upstream_route_degraded");
        assert_eq!(failure.execution.certainty, crate::scheduler::rotation::ExecutionCertainty::Accepted);
        assert!(completed_upstream_response(body, true, None).is_ok());
    }
}
#[tokio::test]
async fn buffered_account_stream_finishes_without_waiting_for_upstream_eof() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0; 1024];
        assert!(socket.read(&mut request).await.unwrap() > 0);
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n").await.unwrap();
        let frame = "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-6-astra\",\"output\":[]}}\n\n";
        socket
            .write_all(format!("{:x}\r\n{frame}\r\n", frame.len()).as_bytes())
            .await
            .unwrap();
        std::future::pending::<()>().await;
    });
    let upstream = reqwest::get(format!("http://{address}/")).await.unwrap();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        super::collect_upstream_response(upstream, true, Some("gpt-6-astra")),
    )
    .await;
    server.abort();
    let bytes = result
        .expect("terminal event must not wait for EOF")
        .unwrap_or_else(|failure| {
            panic!(
                "unexpected buffered response failure: {}",
                failure.failure.category
            )
        });
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["id"],
        "resp_test"
    );
}

#[test]
fn failed_terminal_with_output_does_not_authorize_replay() {
    let payload = serde_json::json!({
        "type": "response.failed",
        "response": {
            "status": "failed",
            "error": {"code": "previous_response_not_found", "message": "previous response not found"},
            "output": [{"type": "message", "content": [{"type": "output_text", "text": "already generated"}]}]
        }
    });
    for bytes in [
        serde_json::to_vec(&payload).unwrap(),
        format!("data: {payload}\n\n").into_bytes(),
    ] {
        let failure = completed_upstream_response(&bytes, true, None).unwrap_err();
        assert_eq!(
            failure.failure.category,
            crate::error_codes::UPSTREAM_PREVIOUS_RESPONSE_NOT_FOUND
        );
        assert_eq!(
            failure.execution.certainty,
            crate::scheduler::rotation::ExecutionCertainty::Accepted
        );
    }
}
