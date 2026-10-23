use super::*;

#[tokio::test]
async fn oversized_native_events_fail_once_and_retain_known_usage() {
    for protocol in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Messages,
        WireApi::Gemini,
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let mut stream =
            usage_stream_with_events(stream::iter([Ok(Bytes::from_static(b"x"))]), events.clone());
        stream.event.as_mut().unwrap().wire_api = protocol;
        stream.event.as_mut().unwrap().input_tokens = Some(7);
        stream.event.as_mut().unwrap().output_tokens = Some(3);
        stream.native_gemini = protocol == WireApi::Gemini;
        stream
            .sse_pending
            .resize(crate::protocol::sse::MAX_EVENT_BYTES, b'x');

        let output = stream.next().await;
        if protocol == WireApi::Responses {
            let failure = parse_sse_event(&output.unwrap().unwrap());
            assert_eq!(failure.outcome, Some(TerminalOutcome::Failure));
            assert_eq!(
                failure.error_category,
                Some(error_codes::STREAM_EVENT_TOO_LARGE)
            );
        } else {
            assert!(output.is_none());
        }
        assert!(stream.terminated);
        assert!(stream.sse_pending.is_empty());
        assert!(stream.next().await.is_none());
        drop(stream);

        let recorded = events.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(!recorded[0].success);
        assert_eq!(
            recorded[0].error_category.as_deref(),
            Some(error_codes::STREAM_EVENT_TOO_LARGE)
        );
        assert_eq!(recorded[0].input_tokens, Some(7));
        assert_eq!(recorded[0].output_tokens, Some(3));
    }
}
