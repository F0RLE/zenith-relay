use super::*;

mod messages_bridge;

fn request() -> Value {
    json!({"model":"test","metadata":{"user_id":"synthetic"},"tools":[{"type":"function","name":"inspect","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}]})
}

fn call(name: &str) -> Value {
    json!({"type":"function_call","id":"fc_native","call_id":"call_1","name":"run_officejs","arguments":json!({"references":[name],"code":"{\"path\":\"test\"}"}).to_string(),"status":"completed"})
}

fn frame(event: Value) -> Bytes {
    Bytes::from(format!("data: {event}\n\n"))
}

fn completed(items: Value) -> Value {
    json!({"type":"response.completed","response":{"id":"resp_test","model":"test","status":"completed","output":items,"usage":{"input_tokens":10,"output_tokens":4,"total_tokens":14}}})
}

fn output(translator: &mut Translator) -> String {
    String::from_utf8(
        translator
            .output
            .drain(..)
            .flat_map(|bytes| bytes.to_vec())
            .collect(),
    )
    .unwrap()
}

#[tokio::test]
async fn text_arrives_before_terminal_tools_are_withheld_and_drop_cancels_reader() {
    let (sender, receiver) = tokio::sync::mpsc::channel(4);
    let inner: UpstreamStream = Box::pin(stream::unfold(receiver, |mut receiver| async {
        receiver.recv().await.map(|bytes| (Ok(bytes), receiver))
    }));
    let mut body = translate_stream(Bytes::new(), inner, request());
    sender
        .send(frame(
            json!({"type":"response.output_text.delta","output_index":0,"delta":"Working"}),
        ))
        .await
        .unwrap();
    let first = tokio::time::timeout(Duration::from_secs(1), body.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&first).contains("Working"));
    sender
        .send(frame(
            json!({"type":"response.output_item.done","output_index":1,"item":call("inspect")}),
        ))
        .await
        .unwrap();
    assert!(tokio::time::timeout(Duration::from_millis(10), body.next())
        .await
        .is_err());
    sender.send(frame(completed(json!([{"type":"message","id":"msg_1","role":"assistant","content":[{"type":"output_text","text":"Working"}]},call("inspect")])))).await.unwrap();
    let mut tail = Vec::new();
    while let Some(chunk) = body.next().await {
        tail.extend_from_slice(&chunk.unwrap());
    }
    let tail = String::from_utf8(tail).unwrap();
    assert!(tail.contains("response.function_call_arguments.delta"));
    assert!(tail.contains("inspect"));
    assert!(!tail.contains("run_officejs"));
    assert!(!tail.contains("response.output_text.delta"));
    assert!(tail.contains("user_id"));
    drop(body);
    assert!(sender.is_closed());
}

#[test]
fn invalid_mixed_or_missing_tools_never_dispatch_a_valid_subset() {
    for response in [
        completed(json!([call("inspect"), call("unknown")])),
        completed(json!([])),
        json!({"type":"response.incomplete","response":{"status":"incomplete","output":[call("inspect")]}}),
        json!({"type":"response.failed","response":{"status":"failed","output":[call("inspect")],"error":{"code":"server_error"}}}),
    ] {
        let mut translator = Translator::new(request());
        translator.push(&frame(
            json!({"type":"response.output_item.done","item":call("inspect")}),
        ));
        translator.push(&frame(response));
        let body = output(&mut translator);
        // Static diagnostic paths may name the transport. No executable item
        // or arguments event may escape, regardless of that error text.
        assert!(!body.contains("\"type\":\"function_call\""));
        assert!(!body.contains("\"type\":\"custom_tool_call\""));
        assert!(!body.contains("function_call_arguments"));
        assert!(!body.contains("\"name\":\"inspect\""));
        assert!(translator.terminal);
    }
}

#[test]
fn fragmented_sse_is_resequenced_and_stops_at_first_terminal() {
    let mut translator = Translator::new(request());
    let source = [frame(json!({"type":"response.created","response":{"metadata":{"task_id":"internal"},"output":[call("inspect")]}})),frame(completed(json!([call("inspect")]))),frame(json!({"type":"response.output_text.delta","delta":"after-terminal"}))].concat();
    for byte in source {
        translator.push(&[byte]);
    }
    let body = output(&mut translator);
    assert!(!body.contains("internal"));
    assert!(!body.contains("after-terminal"));
    let mut bytes = body.into_bytes();
    let mut sequence = 0;
    while let Some(event) = take_sse_event(&mut bytes) {
        let event: Value = serde_json::from_slice(&sse_event_data(&event)).unwrap();
        assert_eq!(event["sequence_number"], sequence);
        sequence += 1;
    }
}

#[test]
fn malformed_frames_and_done_without_completion_fail_closed() {
    for bytes in [b"data: {broken}\n\n".as_slice(), b"data: [DONE]\n\n"] {
        let mut translator = Translator::new(request());
        translator.push(bytes);
        assert!(
            output(&mut translator).contains(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID)
        );
        assert!(translator.terminal);
    }
}

#[test]
fn shared_responses_terminal_alias_and_named_envelopes_complete_without_eof() {
    for event_name in ["response.completed", "response.done"] {
        for named_only in [false, true] {
            let mut translator = Translator::new(request());
            let mut event = completed(json!([call("inspect")]));
            event["type"] = json!(event_name);
            if named_only {
                event.as_object_mut().unwrap().remove("type");
            }
            let wire = format!("event: {event_name}\ndata: {event}\n\n");
            for byte in wire.bytes() {
                translator.push(&[byte]);
            }
            assert!(translator.terminal);
            let body = output(&mut translator);
            assert!(!body.contains(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID));
            assert_eq!(body.matches("event: response.completed").count(), 1);
            assert_eq!(
                body.matches("event: response.function_call_arguments.delta")
                    .count(),
                1
            );
            assert!(!body.contains("run_officejs"));
        }
    }
}

#[test]
fn terminal_alias_does_not_turn_incomplete_or_invalid_output_into_success() {
    for (status, output_items, valid) in [
        ("incomplete", json!([call("inspect")]), true),
        ("completed", json!(null), false),
        ("in_progress", json!([]), false),
    ] {
        let mut translator = Translator::new(request());
        translator.push(&frame(json!({
            "type":"response.done",
            "response":{"status":status,"output":output_items,"usage":{"output_tokens":4}}
        })));
        assert!(translator.terminal);
        let body = output(&mut translator);
        assert!(!body.contains("event: response.completed"));
        assert!(!body.contains("function_call_arguments"));
        assert!(!body.contains("run_officejs"));
        if valid {
            assert!(body.contains("response.incomplete"));
            assert!(body.contains("output_tokens"));
        } else {
            assert!(body.contains(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID));
            assert!(body.contains("response.status"));
        }
    }
    let mut translator = Translator::new(request());
    translator.push(&frame(json!({
        "type":"response.incomplete", "response":{"status":"completed","output":[]}
    })));
    assert!(output(&mut translator).contains(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID));
}

#[test]
fn cancellation_is_terminal_and_never_releases_tools() {
    for kind in ["response.cancelled", "response.canceled"] {
        let mut translator = Translator::new(request());
        translator.push(&frame(json!({
            "type":kind,"response":{"status":"cancelled","output":[call("inspect")]}
        })));
        translator.push(&frame(completed(json!([call("inspect")]))));
        assert!(translator.terminal);
        let body = output(&mut translator);
        assert!(body.contains(kind));
        assert!(!body.contains("response.completed"));
        assert!(!body.contains("run_officejs"));
    }
}

#[test]
fn tool_validation_failures_keep_safe_parameter_in_persisted_diagnostics() {
    let mut translator = Translator::new(request());
    let mut invalid = call("inspect");
    invalid["arguments"] = json!({
        "references":["inspect"],"code":"{\"path\":123,\"private-payload\":\"private-value\"}"
    })
    .to_string()
    .into();
    translator.push(&frame(completed(json!([invalid]))));
    let body = output(&mut translator);
    assert!(body.contains("output.run_officejs.args"));
    assert!(!body.contains("private-payload"));
    assert!(!body.contains("private-value"));
    assert!(!body.contains("response.function_call_arguments"));
    let parsed = super::super::parse_sse_event(body.as_bytes());
    let details = parsed.upstream_error.unwrap();
    assert_eq!(
        details.code.as_deref(),
        Some(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID)
    );
    assert!(details
        .message
        .unwrap()
        .contains("parameter: output.run_officejs.args"));
}

#[test]
fn schema_failure_reports_safe_cause_in_persisted_diagnostics() {
    let frame = failure(
        crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
        Some("output.run_officejs.args.schema"),
    );
    let parsed = super::super::parse_sse_event(&frame);
    let details = parsed.upstream_error.unwrap();
    assert!(details
        .message
        .unwrap()
        .contains("do not match the declared JSON Schema"));
    assert_eq!(
        details.code.as_deref(),
        Some(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID)
    );
}

#[test]
fn raw_model_rejection_keeps_its_category_and_tool_tracking_is_bounded() {
    let mut translator = Translator::new(request());
    translator.push(&frame(
        json!({"error":{"code":crate::error_codes::UPSTREAM_ROUTE_DEGRADED}}),
    ));
    let body = output(&mut translator);
    assert!(body.contains(crate::error_codes::UPSTREAM_ROUTE_DEGRADED));
    assert!(!body.contains(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID));
    assert!(translator.terminal);
    let mut translator = Translator::new(request());
    for index in 0..MAX_TOOLS {
        translator
            .remember_tool(&format!("fc_{index}"), None)
            .unwrap();
    }
    assert!(translator.remember_tool("overflow", None).is_err());
    let mut translator = Translator::new(request());
    translator.push(&vec![b'x'; MAX_FRAME_BYTES + 1]);
    assert!(translator.terminal);
}

#[test]
fn unfinished_or_conflicting_call_identity_never_becomes_success() {
    for first in [
        json!({"type":"response.output_item.added","item":call("inspect")}),
        json!({"type":"response.function_call_arguments.delta","item_id":"fc_native","delta":"{"}),
    ] {
        let mut translator = Translator::new(request());
        translator.push(&frame(first));
        translator.push(&frame(completed(json!([]))));
        let body = output(&mut translator);
        assert!(body.contains(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID));
        assert!(!body.contains("response.completed"));
    }
    let mut translator = Translator::new(request());
    translator.push(&frame(
        json!({"type":"response.output_item.added","item":call("inspect")}),
    ));
    let mut conflicting = call("inspect");
    conflicting["call_id"] = json!("call_other");
    translator.push(&frame(completed(json!([conflicting]))));
    assert!(output(&mut translator).contains(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID));
}

#[tokio::test]
async fn eof_fails_closed_and_terminal_only_text_is_emitted_once() {
    let mut body = translate_stream(Bytes::new(), Box::pin(stream::empty()), request());
    let eof = body.next().await.unwrap().unwrap();
    assert!(
        String::from_utf8_lossy(&eof).contains(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID)
    );
    assert!(body.next().await.is_none());
    let mut translator = Translator::new(request());
    translator.push(&frame(completed(
        json!([{"type":"message","id":"msg_test","role":"assistant",
        "content":[{"type":"output_text","text":"terminal text"}]}]),
    )));
    let body = output(&mut translator);
    assert_eq!(body.matches("event: response.output_text.delta").count(), 1);
    assert_eq!(body.matches("event: response.completed").count(), 1);
}

#[tokio::test(start_paused = true)]
async fn comments_do_not_extend_progress_timeout_but_data_fragments_do() {
    let inner: UpstreamStream = Box::pin(stream::unfold(0, |index| async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        Some((Ok(Bytes::from_static(b": heartbeat\n\n")), index + 1))
    }));
    let mut guarded = guard_progress(inner, Some(Duration::from_secs(3)));
    let start = tokio::time::Instant::now();
    loop {
        let chunk = guarded.next().await.unwrap().unwrap();
        if String::from_utf8_lossy(&chunk).contains(crate::error_codes::STREAM_IDLE_TIMEOUT) {
            break;
        }
    }
    assert_eq!(start.elapsed(), Duration::from_secs(3));
    assert!(guarded.next().await.is_none());
    let inner: UpstreamStream = Box::pin(stream::unfold(0, |index| async move {
        if index == 4 {
            return None;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        Some((Ok(Bytes::from_static(b"data: {")), index + 1))
    }));
    let chunks: Vec<_> = guard_progress(inner, Some(Duration::from_secs(3)))
        .collect()
        .await;
    assert_eq!(chunks.len(), 4);
}
