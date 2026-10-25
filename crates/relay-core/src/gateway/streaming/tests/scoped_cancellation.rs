use super::*;

fn progress(input: u64, output: u64) -> Bytes {
    Bytes::from(format!(
        "data: {{\"type\":\"response.in_progress\",\"response\":{{\"usage\":{{\"input_tokens\":{input},\"output_tokens\":{output},\"total_tokens\":{total}}}}}}}\n\n",
        total = input + output,
    ))
}

#[tokio::test]
async fn cancelling_one_stream_preserves_its_usage_and_does_not_settle_its_peer() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let delta = Bytes::from_static(
        b"data: {\"type\":\"response.output_text.delta\",\"delta\":\"Synthetic output\"}\n\n",
    );
    let first_input = stream::iter([Ok::<_, Infallible>(progress(10, 2)), Ok(delta.clone())])
        .chain(stream::pending());
    let second_input = stream::iter([
        Ok::<_, Infallible>(progress(20, 3)), Ok(delta),
        Ok(Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Synthetic output\"}]}],\"usage\":{\"input_tokens\":20,\"output_tokens\":5,\"total_tokens\":25}}}\n\n")),
    ]);
    let mut first = usage_stream_with_events(first_input, events.clone());
    let mut second = usage_stream_with_events(second_input, events.clone());
    first.event.as_mut().unwrap().request_id = "synthetic-cancelled".into();
    second.event.as_mut().unwrap().request_id = "synthetic-peer".into();
    assert!(first.next().await.unwrap().is_ok());
    assert!(second.next().await.unwrap().is_ok());
    assert!(first.next().await.unwrap().is_ok());
    drop(first);
    {
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].request_id, "synthetic-cancelled");
        assert_eq!(events[0].total_tokens, Some(12));
        assert_eq!(
            events[0].error_category.as_deref(),
            Some(error_codes::CLIENT_CANCELLED)
        );
    }
    while let Some(chunk) = second.next().await {
        chunk.unwrap();
    }
    drop(second);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    let peer = events
        .iter()
        .find(|event| event.request_id == "synthetic-peer")
        .unwrap();
    assert!(peer.success);
    assert_eq!(peer.total_tokens, Some(25));
    assert!(peer.error_category.is_none());
}

#[tokio::test]
async fn eof_after_partial_usage_is_not_a_success_and_never_invents_consumption() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let input = stream::iter([Ok::<_, Infallible>(progress(10, 2))]);
    let mut output = usage_stream_with_events(input, events.clone());
    let mut forwarded = Vec::new();
    while let Some(chunk) = output.next().await {
        forwarded.extend_from_slice(&chunk.unwrap());
    }
    drop(output);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some(error_codes::STREAM_INCOMPLETE)
    );
    assert_eq!(events[0].input_tokens, Some(10));
    assert_eq!(events[0].output_tokens, Some(2));
    assert_eq!(events[0].total_tokens, Some(12));
    // The failure message may mention the missing terminal by name. Check
    // event types rather than confusing diagnostic text with a real terminal.
    let wire = String::from_utf8(forwarded).unwrap();
    let frames: Vec<serde_json::Value> = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|data| serde_json::from_str(data).ok())
        .collect();
    assert!(frames
        .iter()
        .any(|frame| frame["type"] == "response.failed"));
    assert!(frames
        .iter()
        .all(|frame| frame["type"] != "response.completed"));
}
