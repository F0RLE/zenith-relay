use super::*;

#[test]
fn response_incomplete_is_a_terminal_non_failure_outcome() {
    let event = parse_sse_event(
        br#"data: {"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}

"#,
    );
    assert_eq!(event.outcome, Some(TerminalOutcome::Incomplete));
    assert_eq!(event.error_category, Some("response_incomplete"));
}

#[test]
fn all_responses_error_terminal_types_are_failures() {
    for event_type in [
        "response.failed",
        "response.cancelled",
        "response.canceled",
        "error",
    ] {
        let event = format!("data: {{\"type\":\"{event_type}\"}}\n\n");
        assert_eq!(
            parse_sse_event(event.as_bytes()).outcome,
            Some(TerminalOutcome::Failure)
        );
    }
}

#[test]
fn bridge_failure_rewrite_preserves_the_upstream_type_and_event_name() {
    let preserved = PreservedUpstreamError {
        status: StatusCode::SERVICE_UNAVAILABLE,
        category: "upstream_unavailable",
        code: "service_unavailable".into(),
        message: "safe upstream message".into(),
        error_type: None,
    };
    let rewritten = String::from_utf8(rewrite_bridge_failure(
        br#"event: response.cancelled
data: {"type":"response.cancelled","response":{"error":{"type":"invalid_request_error","code":"adapter_upstream_stream_invalid","message":"adapter message"}}}

"#
        .to_vec(),
        Some(&preserved),
    ))
    .unwrap();

    assert!(rewritten.starts_with("event: response.cancelled\ndata: "));
    assert!(rewritten.contains("\"type\":\"server_error\""));
    assert!(rewritten.contains("\"code\":\"service_unavailable\""));
    assert!(rewritten.contains("\"message\":\"safe upstream message\""));
}

type EmptyGeminiUsageStream = UsageStream<futures_util::stream::Empty<Result<Bytes, Infallible>>>;

fn native_gemini_test_stream() -> (EmptyGeminiUsageStream, Arc<Mutex<Vec<UsageEvent>>>) {
    let recorded = Arc::new(Mutex::new(Vec::new()));
    let captured = recorded.clone();
    let mut event = test_usage_event();
    event.wire_api = WireApi::Gemini;
    let stream = UsageStream::new(
        futures_util::stream::empty::<Result<Bytes, Infallible>>(),
        Arc::new(move |event| captured.lock().unwrap().push(event)),
        event,
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );
    (stream, recorded)
}

#[tokio::test]
async fn native_responses_done_marker_without_terminal_is_not_success() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let input = stream::iter([Ok::<_, Infallible>(Bytes::from_static(b"data: [DONE]\n\n"))]);
    let mut stream = usage_stream_with_events(input, events.clone());
    let failure = stream.next().await.unwrap().unwrap();
    let terminal = parse_sse_event(&failure);
    assert_eq!(terminal.outcome, Some(TerminalOutcome::Failure));
    assert_eq!(
        terminal.event_payload.as_ref().unwrap()["response"]["error"]["code"],
        error_codes::STREAM_INCOMPLETE
    );
    assert!(stream.next().await.is_none());
    drop(stream);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some(error_codes::STREAM_INCOMPLETE)
    );
}

#[tokio::test]
async fn foreign_protocol_terminal_markers_never_complete_a_stream() {
    for (wire_api, marker) in [
        (
            WireApi::Responses,
            b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".as_slice(),
        ),
        (
            WireApi::Messages,
            b"event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
        ),
        (
            WireApi::ChatCompletions,
            b"event: response.completed\ndata: {\"type\":\"response.completed\"}\n\n",
        ),
    ] {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let captured = recorded.clone();
        let mut event = test_usage_event();
        event.wire_api = wire_api;
        let input = stream::iter([Ok::<_, Infallible>(Bytes::copy_from_slice(marker))]);
        let mut stream = UsageStream::new(
            input,
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            event,
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        while stream.next().await.is_some() {}
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 1, "{wire_api:?}");
        assert!(!events[0].success, "{wire_api:?}");
        assert_eq!(
            events[0].error_category.as_deref(),
            Some(error_codes::STREAM_INCOMPLETE),
            "{wire_api:?}"
        );
    }
}

#[tokio::test]
async fn native_gemini_error_is_not_promoted_to_success_at_eof() {
    let (mut stream, recorded) = native_gemini_test_stream();
    assert!(stream.ingest_native_gemini(b"data: {\"error\":{\"code\":400,\"status\":\"INVALID_ARGUMENT\",\"message\":\"Invalid field: temperature\"}}\n\n"));
    assert!(stream.terminated);
    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(!recorded[0].success);
    let details = recorded[0].upstream_error.as_ref().unwrap();
    assert_eq!(details.http_status, Some(200));
    assert_eq!(details.code.as_deref(), Some("400"));
    assert_eq!(
        details.message.as_deref(),
        Some("Invalid field: temperature")
    );
}

#[tokio::test]
async fn native_gemini_rejects_malformed_frame_after_valid_output() {
    let (mut stream, recorded) = native_gemini_test_stream();
    assert!(stream.ingest_native_gemini(
        b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n"
    ));
    assert!(!stream.ingest_native_gemini(b"data: {broken\n\n"));
    assert!(stream.terminated);
    let recorded = recorded.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert!(!recorded[0].success);
    assert_eq!(
        recorded[0].error_category.as_deref(),
        Some("stream_invalid")
    );
}

#[tokio::test]
async fn native_gemini_foreign_terminal_markers_do_not_complete_generation() {
    for marker in [
        b"data: [DONE]\n\n".as_slice(),
        b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
    ] {
        let (mut stream, recorded) = native_gemini_test_stream();
        assert!(stream.ingest_native_gemini(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n"
        ));
        assert!(stream.ingest_native_gemini(marker));
        assert!(stream.next().await.is_none());
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(
            events[0].error_category.as_deref(),
            Some("stream_incomplete")
        );
    }
}

#[test]
fn bridge_rewrite_keeps_unknown_provider_codes_and_error_types() {
    let value = json!({"error": {"code": "future_constraint", "type": "future_provider_type", "message": "Constraint check failed"}});
    let preserved = preserved_stream_error(&value).unwrap();
    let bytes = rewrite_bridge_failure(b"data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"adapter_upstream_stream_invalid\"}}}\n\n".to_vec(), Some(&preserved));
    let details = parse_sse_event(&bytes).upstream_error.unwrap();
    assert_eq!(details.code.as_deref(), Some("future_constraint"));
    assert_eq!(details.error_type.as_deref(), Some("future_provider_type"));
    assert_eq!(details.message.as_deref(), Some("Constraint check failed"));
}

#[test]
fn ttft_requires_real_output_for_supported_stream_protocols() {
    for event in [
        "data: {\"type\":\"response.created\"}\n\n",
        "data: {\"choices\":[{\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":null}}\n\n",
    ] {
        assert!(!parse_sse_event(event.as_bytes()).has_output_delta);
    }
    for event in [
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
        "data: {\"type\":\"response.reasoning_text.delta\",\"delta\":\"thinking\"}\n\n",
        "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"summary\"}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"PowerShell\"}}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
        "data: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hello\"}}\n\n",
        "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"hello\"}]}}]}\n\n",
    ] {
        assert!(parse_sse_event(event.as_bytes()).has_output_delta);
    }
}

#[test]
fn streamed_custom_tool_input_commits_the_response() {
    let event = parse_sse_event(
        br#"data: {"type":"response.custom_tool_call_input.delta","delta":"{"}

"#,
    );

    assert!(event.has_output_delta);
}
