use super::*;

#[test]
fn multiline_json_keeps_data_lines_with_all_sse_line_endings() {
    for ending in ["\n", "\r\n", "\r"] {
        let frame = format!("event: response.output_text.delta{ending}data: {{\"type\":{ending}data: \"response.output_text.delta\",{ending}data: \"delta\":\"synthetic\"}}{ending}{ending}");
        let event = parse_sse_event(frame.as_bytes());
        assert!(event.has_data && event.valid && event.semantic_output);
        assert_eq!(event.payload.unwrap()["delta"], "synthetic");
    }
}

#[test]
fn repeated_events_with_mixed_cr_separators_are_not_one_json_payload() {
    let mut bytes = Vec::new();
    for index in 0..5 {
        let ending = if index == 4 { "\n\n" } else { "\n\r" };
        bytes.extend_from_slice(format!("event: response.created\ndata: {{\"type\":\"response.created\",\"sequence_number\":{index}}}{ending}").as_bytes());
    }
    let mut count = 0;
    while let Some(frame) = crate::protocol::take_sse_event(&mut bytes) {
        let event = parse_sse_event(&frame);
        assert!(event.valid);
        assert_eq!(event.payload.unwrap()["sequence_number"], count);
        count += 1;
    }
    assert_eq!(count, 5);
    assert!(bytes.is_empty());
}

#[test]
fn raw_responses_compaction_delta_is_forwarded_without_stream_invalid() {
    let raw = b"encrypted-compaction-fragment";
    let event = parse_sse_event(
        b"event: response.compaction.delta\ndata: encrypted-compaction-fragment\n\n",
    );

    assert!(event.has_data);
    assert!(event.valid);
    assert_eq!(event.outcome, None);
    assert_eq!(event.raw_data.as_deref(), Some(raw.as_slice()));
}

#[test]
fn json_responses_compaction_data_keeps_the_original_bytes() {
    let event = parse_sse_event(
        b"event: response.compaction.delta\ndata: { \"type\": \"response.compaction.delta\", \"opaque\": true }\n\n",
    );

    assert!(event.valid);
    assert_eq!(
        event.raw_data.as_deref(),
        Some(b"{ \"type\": \"response.compaction.delta\", \"opaque\": true }".as_slice())
    );
    assert_eq!(
        event.payload.as_ref().and_then(|value| value.get("opaque")),
        Some(&Value::Bool(true))
    );
}

#[test]
fn compaction_output_items_are_opaque_non_output_events() {
    for item_type in ["compaction", "compaction_summary"] {
        let event = parse_sse_event(
            format!(
                "event: response.output_item.done\ndata: {{\"type\":\"response.output_item.done\",\"item\":{{\"type\":\"{item_type}\",\"encrypted_content\":\"opaque\"}}}}\n\n"
            )
            .as_bytes(),
        );

        assert!(event.valid);
        assert!(event.is_compaction);
        assert!(!event.semantic_output);
        assert!(event.output_item.is_some());
        assert!(event.raw_data.is_some());
    }
}

#[test]
fn completed_non_compaction_output_item_commits_generation() {
    let event = parse_sse_event(
        b"data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"content\":[]}}\n\n",
    );

    assert!(event.valid);
    assert!(!event.is_compaction);
    assert!(event.semantic_output);
}

#[test]
fn malformed_non_compaction_event_remains_invalid() {
    let event = parse_sse_event(b"event: response.output_text.delta\ndata: {broken\n\n");

    assert!(event.has_data);
    assert!(!event.valid);
}

#[test]
fn completed_event_cannot_override_an_explicit_noncompleted_response_status() {
    for (status, expected) in [
        ("failed", TerminalOutcome::Failure),
        ("incomplete", TerminalOutcome::Incomplete),
        ("in_progress", TerminalOutcome::Failure),
    ] {
        let data = serde_json::json!({
            "type": "response.completed",
            "response": {"status": status}
        });
        let event = parse_sse_event(format!("data: {data}\n\n").as_bytes());
        assert_eq!(event.outcome, Some(expected), "{status}");
        assert!(event.error_category.is_some(), "{status}");
    }
}

#[test]
fn gemini_filter_and_token_limit_are_incomplete_but_unknown_terminal_is_not() {
    for data in [
        r#"{"promptFeedback":{"blockReason":"SAFETY"},"candidates":[]}"#,
        r#"{"candidates":[{"finishReason":"SAFETY"}]}"#,
        r#"{"candidates":[{"finishReason":"MAX_TOKENS"}]}"#,
    ] {
        let event = parse_sse_event(format!("data: {data}\n\n").as_bytes());
        assert_eq!(event.outcome, Some(TerminalOutcome::Incomplete), "{data}");
        assert_eq!(event.error_category, Some(error_codes::RESPONSE_INCOMPLETE));
    }
    for data in [
        r#"{"candidates":[]}"#,
        r#"{"candidates":[{"finishReason":"NEW_REASON"}]}"#,
        r#"{"promptFeedback":{"blockReason":"NEW_REASON"}}"#,
        r#"{"promptFeedback":{"blockReason":"NEW_REASON"},"candidates":[{"finishReason":"SAFETY"}]}"#,
    ] {
        let event = parse_sse_event(format!("data: {data}\n\n").as_bytes());
        assert_eq!(event.outcome, None, "{data}");
    }
    let with_error = parse_sse_event(
        br#"data: {"error":{"code":500},"candidates":[{"finishReason":"SAFETY"}]}

"#,
    );
    assert_eq!(with_error.outcome, Some(TerminalOutcome::Failure));
}

#[test]
fn native_gemini_media_and_code_parts_are_semantic_output() {
    for part in [
        serde_json::json!({"inlineData":{"mimeType":"image/png","data":"YQ=="}}),
        serde_json::json!({"fileData":{"mimeType":"image/png","fileUri":"gs://example/image"}}),
        serde_json::json!({"executableCode":{"language":"PYTHON","code":"print(1)"}}),
        serde_json::json!({"codeExecutionResult":{"outcome":"OUTCOME_OK","output":"1"}}),
    ] {
        let frame = format!(
            "data: {}\n\n",
            serde_json::json!({"candidates":[{"content":{"parts":[part]}}]})
        );
        assert!(parse_sse_event(frame.as_bytes()).semantic_output);
    }
    let metadata = serde_json::json!({
        "candidates":[{"content":{"parts":[{"thoughtSignature":"opaque"}]},"finishReason":"STOP"}]
    });
    let metadata_only = parse_sse_event(format!("data: {metadata}\n\n").as_bytes());
    assert!(!metadata_only.semantic_output);
}

#[test]
fn empty_incomplete_requires_explicit_zero_tokens_and_no_output() {
    let empty = serde_json::json!({
        "type": "response.incomplete",
        "response": {"output": [], "usage": {"output_tokens": 0}}
    });
    assert!(is_empty_responses_incomplete(&empty, false, 0));

    let decimal_zero = serde_json::json!({
        "type": "response.incomplete",
        "response": {"output": [], "usage": {"output_tokens": 0.0}}
    });
    assert!(!is_empty_responses_incomplete(&decimal_zero, false, 0));
    assert!(!is_empty_responses_incomplete(&empty, true, 0));
    assert!(!is_empty_responses_incomplete(&empty, false, 1));
}
