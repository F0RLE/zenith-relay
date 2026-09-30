use super::*;

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
async fn usage_stream_does_not_append_a_synthetic_failure_after_visible_bytes() {
    let partial = [
        Bytes::from_static(
            b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        ),
        Bytes::from_static(
            b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
        ),
        Bytes::from_static(
            b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\"}}\n\n",
        ),
        Bytes::from_static(br#"data: {"type":"response.output_text.delta","delta":"par"#),
    ];
    let cases = partial
        .into_iter()
        .map(|first| (vec![first], "stream_incomplete"))
        .chain(std::iter::once((
            vec![
                Bytes::from_static(
                    b"data: {\"type\":\"response.function_call_arguments.delta\",\"delta\":\"{\"}\n\n",
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
    let first = Bytes::from_static(
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
    );
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
        failure.payload.unwrap()["response"]["error"]["code"],
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
