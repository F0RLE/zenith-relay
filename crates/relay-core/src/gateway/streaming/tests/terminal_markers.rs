use super::*;

#[test]
fn response_incomplete_is_a_terminal_non_failure_outcome() {
    let event = parse_sse_event(
        br#"data: {"type":"response.incomplete","response":{"status":"incomplete","output":[],"incomplete_details":{"reason":"max_output_tokens"}}}

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
async fn interactions_interruptions_keep_partial_usage_and_emit_native_errors() {
    for ending in ["", "data: [DONE]\n\n", "data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"requires_action\"}}\n\n", "data: {\"event_type\":\"interaction.completed\",\"interaction\":{\"status\":\"requires_action\",\"outputs\":[]}}\n\n"] {
        let frames = format!("data: {{\"event_type\":\"interaction.created\",\"interaction\":{{\"id\":\"synthetic\",\"status\":\"in_progress\",\"usage\":{{\"total_input_tokens\":7,\"total_output_tokens\":2}}}}}}\n\ndata: {{\"event_type\":\"content.delta\",\"delta\":{{\"type\":\"text\",\"text\":\"partial\"}}}}\n\n{ending}");
        let events = Arc::new(Mutex::new(Vec::new()));
        let capture = events.clone();
        let mut event = test_usage_event();
        event.wire_api = WireApi::GeminiInteractions;
        let input = stream::iter([Ok::<_, Infallible>(Bytes::from(frames))]);
        let mut stream = UsageStream::new(input, Arc::new(move |event| capture.lock().unwrap().push(event)), event, Instant::now(), Arc::new(|_, _, _| {}));
        let mut output = Vec::new();
        while let Some(chunk) = stream.next().await { output.extend_from_slice(&chunk.unwrap()); }
        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("\"event_type\":\"error\""), "{output}");
        assert!(!output.contains("response.failed"));
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(events[0].input_tokens, Some(7));
        assert_eq!(events[0].output_tokens, Some(2));
    }
}

#[tokio::test]
async fn native_streams_forward_provider_extensions_without_using_them_as_completion() {
    for (protocol, frames) in [
        (
            WireApi::Messages,
            concat!(
                "data: {\"type\":\"message_start\",\"message\":{\"id\":\"synthetic-message\",\"content\":[]}}\n\n",
                "data: {\"type\":\"vendor_progress\",\"vendor_extension\":{\"step\":1}}\n\n",
                "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"answer\"}}\n\n",
                "data: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n",
                "data: {\"type\":\"message_stop\"}\n\n",
            ),
        ),
        (
            WireApi::Gemini,
            concat!(
                "data: {\"type\":\"vendor_progress\",\"vendor_extension\":{\"step\":1}}\n\n",
                "data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"answer\",\"vendor_extension\":true}]},\"finishReason\":\"STOP\"}]}\n\n",
            ),
        ),
    ] {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let capture = recorded.clone();
        let mut event = test_usage_event();
        event.wire_api = protocol;
        let input = stream::iter([Ok::<_, Infallible>(Bytes::from_static(frames.as_bytes()))]);
        let mut stream = UsageStream::new(
            input,
            Arc::new(move |event| capture.lock().unwrap().push(event)),
            event,
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        let mut output = Vec::new();
        while let Some(chunk) = stream.next().await {
            output.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(output, frames.as_bytes(), "{protocol:?}");
        let recorded = recorded.lock().unwrap();
        assert_eq!(recorded.len(), 1, "{protocol:?}");
        assert!(recorded[0].success, "{protocol:?}");
    }
}

#[tokio::test]
async fn native_responses_sse_keeps_wire_bytes_across_chunk_boundaries() {
    let frames = concat!(
        ": synthetic heartbeat\r\n\r\n",
        "id: synthetic-1\r\nretry: 1500\r\nevent: response.created\r\n",
        "data: {\"type\":\"response.created\",\"response\":{\"id\":\"synthetic-response\"}}\r\n\r\n",
        "event: vendor_extension\r\ndata: {\"type\":\"vendor_extension\",\"z\":1e3,\"a\":123456789012345678901234567890,\"p\":0.12345678901234567890123456789}\r\n\r\n",
        "event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"ответ\"}\r\n\r\n",
        "event: response.completed\r\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"synthetic-response\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":4,\"output_tokens\":2}}}\r\n\r\n",
    );
    // Responses stops at the protocol terminal event, not transport EOF or
    // [DONE]. A CR already terminates the blank line; an optional LF in the
    // next chunk is outside the completed event and is not awaited.
    let input_frames = format!("{frames}data: [DONE]\r\n\r\ndata: ignored\n\n");
    for chunk_size in [1, 7, frames.len()] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let input = stream::iter(
            input_frames
                .as_bytes()
                .chunks(chunk_size)
                .map(|chunk| Ok::<_, Infallible>(Bytes::copy_from_slice(chunk)))
                .collect::<Vec<_>>(),
        );
        let mut stream = usage_stream_with_events(input, events.clone());
        let mut output = Vec::new();
        while let Some(chunk) = stream.next().await {
            output.extend_from_slice(&chunk.unwrap());
        }
        let terminal_end = if (frames.len() - 1).is_multiple_of(chunk_size) {
            frames.len() - 1
        } else {
            frames.len()
        };
        assert_eq!(
            output,
            &frames.as_bytes()[..terminal_end],
            "chunk size {chunk_size}"
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
    }
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
            Some(error_codes::STREAM_INVALID),
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
    for (marker, category) in [
        (b"data: [DONE]\n\n".as_slice(), "stream_incomplete"),
        (b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n".as_slice(), "stream_invalid"),
    ] {
        let (mut stream, recorded) = native_gemini_test_stream();
        assert!(stream.ingest_native_gemini(
            b"data: {\"candidates\":[{\"content\":{\"parts\":[{\"text\":\"partial\"}]}}]}\n\n"
        ));
        assert!(!stream.ingest_native_gemini(marker));
        assert!(stream.next().await.is_none());
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(
            events[0].error_category.as_deref(),
            Some(category)
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
