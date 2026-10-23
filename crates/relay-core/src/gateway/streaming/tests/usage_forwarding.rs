use super::*;

#[tokio::test]
async fn normalized_output_timing_survives_raw_timing_settlement_without_inventing_readiness() {
    for outcome in ["completed", "empty", "invalid", "cancelled"] {
        let mut event = test_usage_event();
        event.routing = Some(
            serde_json::from_value(json!({
                "reason":"only_eligible", "eligibleCandidates":1,
                "inFlightBefore":0, "dispatchesBefore":0,
                "transport":{"upstream":"http", "mode":"basis_points"}
            }))
            .unwrap(),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut input = vec![Bytes::from_static(b": heartbeat\n\n")];
        if outcome != "empty" {
            input.push(Bytes::from_static(
                b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
            ));
        }
        if outcome == "invalid" {
            input.push(Bytes::from_static(b"data: invalid\n\n"));
        } else if outcome != "cancelled" {
            input.push(Bytes::from_static(
                b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[]}}\n\n",
            ));
        }
        let chunks = input.len();
        let mut stream = UsageStream::new(
            stream::iter(input.into_iter().map(Ok::<Bytes, Infallible>)),
            Arc::new(move |event| captured.lock().unwrap().push(event)),
            event,
            Instant::now() - std::time::Duration::from_millis(100),
            Arc::new(|event, _, _| {
                let mut raw = crate::gateway::response::BufferedResponseTiming::default();
                raw.observe(&parse_sse_event(b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{}\"}\n\n"), 5);
                raw.finish(50);
                raw.apply(event);
            }),
        );
        for _ in 0..chunks {
            assert!(stream.next().await.unwrap().is_ok());
        }
        drop(stream);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.ttft_ms, Some(5));
        assert_eq!(event.latency_ms, 50);
        let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(transport.upstream_response_ms, Some(50));
        assert_eq!(
            transport.client_first_output_ms.is_some(),
            outcome != "empty"
        );
        if let Some(first) = transport.client_first_output_ms {
            assert!(first >= 100);
        }
        assert_eq!(
            transport.client_response_ready_ms.is_some(),
            matches!(outcome, "completed" | "empty")
        );
        if let Some(ready) = transport.client_response_ready_ms {
            assert!(ready >= transport.client_first_output_ms.unwrap_or(100));
        }
    }
}

#[tokio::test]
async fn native_gemini_continuation_retains_its_private_checkpoint_and_late_usage() {
    let terminal = Bytes::from_static(b"data: {\"candidates\":[{\"index\":0,\"finishReason\":\"CONTINUATION\",\"continuationToken\":\"c3ludGhldGlj\",\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"Partial answer\"}]}}]}\n\n");
    let usage = Bytes::from_static(b"data: {\"usageMetadata\":{\"promptTokenCount\":5,\"candidatesTokenCount\":2,\"totalTokenCount\":7}}\n\n");
    let second = Bytes::from_static(b"data: {\"candidates\":[{\"index\":1,\"finishReason\":\"CONTINUATION\",\"continuationToken\":\"c2Vjb25k\"}]}\n\n");
    let expected = [terminal.as_ref(), second.as_ref(), usage.as_ref()].concat();
    let events = Arc::new(Mutex::new(Vec::new()));
    let checkpoint = Arc::new(Mutex::new(None));
    let mut event = test_usage_event();
    event.wire_api = WireApi::Gemini;
    let captured = events.clone();
    let mut stream = UsageStream::new(
        stream::iter([Ok::<_, Infallible>(terminal), Ok(second), Ok(usage)]),
        Arc::new(move |event| captured.lock().unwrap().push(event)),
        event,
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );
    stream.native_response = Some(checkpoint.clone());
    let mut wire = Vec::new();
    while let Some(chunk) = stream.next().await {
        wire.extend_from_slice(&chunk.unwrap());
    }
    assert_eq!(wire, expected);
    assert_eq!(
        checkpoint.lock().unwrap().as_ref().unwrap()["candidates"][0]["continuationToken"],
        "c3ludGhldGlj"
    );
    assert_eq!(
        checkpoint.lock().unwrap().as_ref().unwrap()["candidates"][1]["continuationToken"],
        "c2Vjb25k"
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some(error_codes::RESPONSE_INCOMPLETE)
    );
    assert_eq!(events[0].input_tokens, Some(5));
    assert_eq!(events[0].output_tokens, Some(2));
    assert!(!format!("{:?}", events[0].upstream_error).contains("c3ludGhldGlj"));
}

#[tokio::test]
async fn late_model_substitution_ends_the_stream_without_forwarding_completion() {
    let first = Bytes::from_static(
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"synthetic\"}\n\n",
    );
    let completed = Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-5.6-luna\",\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n");
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut stream = usage_stream_with_events(
        stream::iter([Ok::<_, Infallible>(first.clone()), Ok(completed)]),
        events.clone(),
    );
    stream.expected_model = Some("gpt-6-astra".into());
    assert_eq!(stream.next().await.unwrap().unwrap(), first);
    let failure = parse_sse_event(&stream.next().await.unwrap().unwrap());
    assert_eq!(failure.outcome, Some(TerminalOutcome::Failure));
    assert_eq!(
        failure.event_payload.unwrap()["response"]["error"]["code"],
        error_codes::UPSTREAM_ROUTE_DEGRADED
    );
    assert!(stream.next().await.is_none());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].input_tokens, Some(3));
    assert_eq!(events[0].output_tokens, Some(2));
    assert_eq!(events[0].total_tokens, Some(5));
    assert_eq!(
        events[0].error_category.as_deref(),
        Some(error_codes::UPSTREAM_ROUTE_DEGRADED)
    );
}

#[tokio::test]
async fn usage_stream_forwards_chunks_without_waiting_for_an_sse_boundary() {
    let first = Bytes::from_static(br#"data: {"type":"response.output_text.delta","delta":"hel"#);
    let second = Bytes::from_static(b"lo\"}\n\n");
    let completed = Bytes::from_static(
        b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\"}}\n\n",
    );
    let input = futures_util::stream::iter([
        Ok::<_, Infallible>(first.clone()),
        Ok(second.clone()),
        Ok(completed.clone()),
    ]);
    let mut stream = UsageStream::new(
        input,
        Arc::new(|_| {}),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );

    assert_eq!(stream.next().await.unwrap().unwrap(), first);
    assert_eq!(stream.next().await.unwrap().unwrap(), second);
    assert_eq!(stream.next().await.unwrap().unwrap(), completed);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn response_delta_is_delivered_while_upstream_remains_pending() {
    let delta = Bytes::from_static(
        b"event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n",
    );
    let input = stream::iter([Ok::<_, Infallible>(delta.clone())]).chain(stream::pending());
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut stream = usage_stream_with_events(input, events.clone());
    let next = tokio::time::timeout(std::time::Duration::from_secs(1), stream.next())
        .await
        .expect("output must not wait for another upstream chunk");
    assert_eq!(next.unwrap().unwrap(), delta);
    assert!(events.lock().unwrap().is_empty());
    drop(stream);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].ttft_ms.is_some());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some(error_codes::CLIENT_CANCELLED)
    );
}

#[tokio::test]
async fn usage_stream_does_not_append_a_failure_inside_a_visible_frame() {
    let partial = [
        Bytes::from_static(br#"data: {"type":"response.function_call_arguments.delta","delta":"{"#),
        Bytes::from_static(br#"data: {"type":"response.created","response":{"id":"resp_"#),
        Bytes::from_static(br#"data: {"type":"response.output_text.delta","delta":"par"#),
    ];
    let cases = partial
        .into_iter()
        .map(|first| (vec![first], "stream_incomplete"))
        .chain(std::iter::once((
            vec![
                Bytes::from_static(
                    br#"data: {"type":"response.function_call_arguments.delta","delta":"{"#,
                ),
                Bytes::from_static(b"data: invalid-json\n\n"),
            ],
            "stream_invalid",
        )));
    for (input, category) in cases {
        let first = input[0].clone();
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut stream = usage_stream_with_events(
            stream::iter(input.into_iter().map(Ok::<Bytes, Infallible>)),
            events.clone(),
        );

        assert_eq!(stream.next().await.unwrap().unwrap(), first);
        assert!(stream.next().await.is_none());
        drop(stream);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(events[0].error_category.as_deref(), Some(category));
        if category == error_codes::STREAM_INVALID {
            let details = events[0].upstream_error.as_ref().unwrap();
            assert_eq!(details.error_type.as_deref(), Some("relay_stream_parser"));
            assert!(!details.message.as_ref().unwrap().contains("invalid-json"));
        }
    }
}

#[tokio::test]
async fn usage_stream_preserves_transport_errors_after_partial_output() {
    let first = Bytes::from_static(br#"data: {"type":"response.output_text.delta","delta":"par"#);
    let mut stream = UsageStream::new(
        stream::iter([
            Ok(first.clone()),
            Err(std::io::Error::from(std::io::ErrorKind::ConnectionReset)),
        ]),
        Arc::new(|_| {}),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );

    assert_eq!(stream.next().await.unwrap().unwrap(), first);
    assert_eq!(
        stream.next().await.unwrap().unwrap_err().kind(),
        std::io::ErrorKind::ConnectionReset
    );
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn usage_stream_reports_interruption_after_complete_visible_frames() {
    let first = Bytes::from_static(
        concat!(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_interrupted\"}}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"sequence_number\":9,\"delta\":\"partial\"}\n\n",
        )
        .as_bytes(),
    );
    for (tail, category) in [
        (None, error_codes::STREAM_INCOMPLETE),
        (
            Some(Err(std::io::Error::from(
                std::io::ErrorKind::ConnectionReset,
            ))),
            error_codes::UPSTREAM_STREAM,
        ),
        (
            Some(Ok(Bytes::from_static(b"data: invalid-json\n\n"))),
            error_codes::STREAM_INVALID,
        ),
    ] {
        let recorded = Arc::new(Mutex::new(Vec::new()));
        let capture = recorded.clone();
        let input = std::iter::once(Ok(first.clone())).chain(tail);
        let mut stream = UsageStream::new(
            stream::iter(input),
            Arc::new(move |event| capture.lock().unwrap().push(event)),
            test_usage_event(),
            Instant::now(),
            Arc::new(|_, _, _| {}),
        );
        assert_eq!(stream.next().await.unwrap().unwrap(), first);
        let bytes = stream.next().await.unwrap().unwrap();
        let terminal = parse_sse_event(&bytes);
        assert_eq!(terminal.outcome, Some(TerminalOutcome::Failure));
        assert_eq!(
            terminal.event_payload.as_ref().unwrap()["sequence_number"],
            10
        );
        let response = &terminal.event_payload.as_ref().unwrap()["response"];
        assert_eq!(response["id"], "resp_interrupted");
        assert_eq!(response["status"], "failed");
        assert_eq!(response["error"]["code"], category);
        assert!(!String::from_utf8_lossy(&bytes).contains("event: response.completed\n"));
        assert!(stream.next().await.is_none());
        drop(stream);
        let events = recorded.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        assert_eq!(events[0].error_category.as_deref(), Some(category));
        if category == error_codes::UPSTREAM_STREAM {
            let details = events[0].upstream_error.as_ref().unwrap();
            assert_eq!(
                details.error_type.as_deref(),
                Some("relay_stream_transport")
            );
            assert!(details
                .message
                .as_ref()
                .unwrap()
                .contains("io=connection_reset"));
        }
    }
}

#[tokio::test]
async fn usage_stream_keeps_failure_at_a_boundary_after_fragmented_events() {
    for ending in ["\n", "\r\n", "\r"] {
        let frames = format!(
            "data: {{\"type\":\"response.created\",\"sequence_number\":11,\"response\":{{\"id\":\"resp_fragmented\"}}}}{ending}{ending}data: {{\"type\":\"response.output_text.delta\",\"sequence_number\":19,\"delta\":\"synthetic\"}}{ending}{ending}"
        );
        for size in [1, 7, frames.len()] {
            let input = stream::iter(
                frames
                    .as_bytes()
                    .chunks(size)
                    .map(|chunk| Ok::<_, Infallible>(Bytes::copy_from_slice(chunk))),
            );
            let events = Arc::new(Mutex::new(Vec::new()));
            let mut stream = usage_stream_with_events(input, events.clone());
            let mut output = Vec::new();
            while let Some(bytes) = stream.next().await {
                output.extend_from_slice(&bytes.unwrap());
            }
            assert!(output.starts_with(frames.as_bytes()), "{ending:?}, {size}");
            let failure = parse_sse_event(&output[frames.len()..]);
            assert_eq!(failure.outcome, Some(TerminalOutcome::Failure));
            let payload = failure.event_payload.unwrap();
            assert_eq!(payload["type"], "response.failed");
            assert_eq!(payload["sequence_number"], 20);
            assert_eq!(payload["response"]["id"], "resp_fragmented");
            assert_eq!(events.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn usage_stream_still_reports_a_failure_before_any_visible_bytes() {
    let mut stream = UsageStream::new(
        stream::empty::<Result<Bytes, Infallible>>(),
        Arc::new(|_| {}),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );

    let bytes = stream.next().await.unwrap().unwrap();
    let failure = parse_sse_event(&bytes);
    assert_eq!(failure.outcome, Some(TerminalOutcome::Failure));
    assert_eq!(
        failure.event_payload.unwrap()["response"]["error"]["code"],
        "stream_incomplete"
    );
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn usage_stream_preserves_upstream_terminal_failures_after_output() {
    let first = Bytes::from_static(
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
    );
    let failure = Bytes::from_static(
        b"data: {\"type\":\"response.failed\",\"response\":{\"id\":\"resp_test\",\"error\":{\"code\":\"server_error\"}}}\n\n",
    );
    let mut stream = UsageStream::new(
        stream::iter([Ok::<_, Infallible>(first.clone()), Ok(failure.clone())]),
        Arc::new(|_| {}),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );

    assert_eq!(stream.next().await.unwrap().unwrap(), first);
    assert_eq!(stream.next().await.unwrap().unwrap(), failure);
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn upstream_stream_errors_prefix_the_selected_account() {
    let mut event = test_usage_event();
    event.account_id = Some("synthetic-account".to_string());
    let first = Bytes::from_static(
        b"event: response.failed\ndata: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"server_error\",\"message\":\"",
    );
    let second = Bytes::from_static(b"connection closed\"}}}\n\n");
    let mut stream = UsageStream::new(
        stream::iter([Ok::<_, Infallible>(first), Ok(second)]),
        Arc::new(|_| {}),
        event,
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );

    let error = stream.next().await.unwrap().unwrap();
    let error = parse_sse_event(&error);
    assert_eq!(
        error.event_payload.unwrap()["response"]["error"]["message"],
        "Account: connection closed"
    );
}

#[tokio::test]
async fn upstream_stream_error_with_reordered_json_fields_gets_prefixed_as_one_event() {
    let mut event = test_usage_event();
    event.account_id = Some("synthetic-account".to_string());
    let first = Bytes::from_static(
        b"event: response.failed\ndata: {\"id\":\"resp_test\",\"type\":\"response.failed\",",
    );
    let second =
        Bytes::from_static(b"\"response\":{\"error\":{\"message\":\"connection closed\"}}}\n\n");
    let mut stream = UsageStream::new(
        stream::iter([Ok::<_, Infallible>(first), Ok(second)]),
        Arc::new(|_| {}),
        event,
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );

    let forwarded = stream.next().await.unwrap().unwrap();
    let error = parse_sse_event(&forwarded);
    let payload = error.event_payload.unwrap();
    assert_eq!(payload["id"], "resp_test");
    assert_eq!(
        payload["response"]["error"]["message"],
        "Account: connection closed"
    );
    assert!(stream.next().await.is_none());
}

#[tokio::test]
async fn upstream_sse_error_event_without_json_type_gets_prefixed() {
    let mut event = test_usage_event();
    event.account_id = Some("synthetic-account".to_string());
    let error = Bytes::from_static(b"event: error\ndata: {\"message\":\"connection closed\"}\n\n");
    let mut stream = UsageStream::new(
        stream::iter([Ok::<_, Infallible>(error)]),
        Arc::new(|_| {}),
        event,
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );

    let forwarded = stream.next().await.unwrap().unwrap();
    let error = parse_sse_event(&forwarded);
    assert_eq!(error.outcome, Some(TerminalOutcome::Failure));
    assert_eq!(
        error.event_payload.unwrap()["message"],
        "Account: connection closed"
    );
}

#[tokio::test]
async fn native_responses_stream_capture_keeps_completed_tool_output_for_http_replay() {
    let captured = Arc::new(Mutex::new(None));
    let mut stream = UsageStream::new(
        futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
        Arc::new(|_| {}),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );
    stream.native_response = Some(captured.clone());
    stream.ingest_sse(
        b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"call_id\":\"call_stream_01\",\"name\":\"run_command\",\"arguments\":\"{\\\"command\\\":\\\"pwd\\\"}\"}}\n\n",
    );
    stream.ingest_sse(
        b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_stream_01\",\"status\":\"completed\",\"output\":[]}}\n\n",
    );

    let response = captured
        .lock()
        .unwrap()
        .clone()
        .expect("completed native stream is captured");
    assert_eq!(response["id"], "resp_stream_01");
    assert_eq!(response["output"][0]["type"], "function_call");
    assert_eq!(response["output"][0]["call_id"], "call_stream_01");
}

#[tokio::test]
async fn incomplete_native_response_is_captured_without_gateway_failure_status() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = events.clone();
    let captured_response = Arc::new(Mutex::new(None));
    let mut stream = UsageStream::new(
        futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
        Arc::new(move |event| captured_events.lock().unwrap().push(event)),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );
    stream.native_response = Some(captured_response.clone());
    stream.ingest_sse(
        br#"data: {"type":"response.incomplete","response":{"id":"resp_incomplete","status":"incomplete","incomplete_details":{"reason":"max_output_tokens"},"output":[],"usage":{"input_tokens":3,"output_tokens":4}}}

"#,
    );

    let response = captured_response
        .lock()
        .unwrap()
        .clone()
        .expect("incomplete native stream is captured");
    assert_eq!(response["id"], "resp_incomplete");
    assert_eq!(
        response["incomplete_details"]["reason"],
        "max_output_tokens"
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].http_status, StatusCode::OK.as_u16());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("response_incomplete")
    );
    assert_eq!(events[0].output_tokens, Some(4));
}

#[tokio::test]
async fn streaming_chat_usage_captures_cached_prompt_tokens() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let mut stream = UsageStream::new(
        futures_util::stream::empty::<std::result::Result<Bytes, Infallible>>(),
        Arc::new(move |event| captured.lock().unwrap().push(event)),
        test_usage_event(),
        Instant::now(),
        Arc::new(|_, _, _| {}),
    );
    stream.ingest_sse(
        b"data: {\"type\":\"response.completed\",\"response\":{\"service_tier\":\"default\",\"usage\":{\"prompt_tokens\":32,\"prompt_tokens_details\":{\"cached_tokens\":9,\"cache_write_tokens\":7},\"completion_tokens\":6,\"completion_tokens_details\":{\"reasoning_tokens\":4}}}}\n\n",
    );

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].input_tokens, Some(32));
    assert_eq!(events[0].cached_input_tokens, Some(9));
    assert_eq!(events[0].cache_write_input_tokens, Some(7));
    assert_eq!(events[0].reasoning_tokens, Some(4));
    assert_eq!(events[0].output_tokens, Some(6));
    assert_eq!(events[0].total_tokens, Some(38));
    assert_eq!(events[0].applied_service_tier, Some("default".to_string()));
}

#[tokio::test]
async fn response_deltas_forward_immediately_and_keep_terminal_usage() {
    let empty =
        Bytes::from_static(b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"\"}\n\n");
    let delta = Bytes::from_static(
        b"data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
    );
    let done = Bytes::from_static(
        b"data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}],\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n",
    );
    let mut combined = Vec::new();
    combined.extend_from_slice(&empty);
    combined.extend_from_slice(&delta);
    combined.extend_from_slice(&done);

    for input in [
        vec![
            Ok::<_, Infallible>(empty.clone()),
            Ok(delta.clone()),
            Ok(done.clone()),
        ],
        combined
            .chunks(5)
            .map(|chunk| Ok(Bytes::copy_from_slice(chunk)))
            .collect::<Vec<_>>(),
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut stream = usage_stream_with_events(stream::iter(input), events.clone());
        let mut forwarded = Vec::new();
        while let Some(chunk) = stream.next().await {
            forwarded.extend_from_slice(&chunk.unwrap());
        }
        assert_eq!(forwarded, combined);
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert!(events[0].success);
        assert!(events[0].ttft_ms.is_some());
        assert_eq!(events[0].input_tokens, Some(3));
        assert_eq!(events[0].output_tokens, Some(2));
        assert_eq!(events[0].total_tokens, Some(5));
    }
}
