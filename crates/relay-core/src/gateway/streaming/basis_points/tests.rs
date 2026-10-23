use super::*;
use crate::protocol::take_sse_event;

mod batch;
mod messages_bridge;
mod throughput;

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
fn streamed_parallel_calls_keep_order_and_honor_an_explicit_serial_policy() {
    for parallel in [None, Some(true), Some(false)] {
        let mut request = request();
        if let Some(parallel) = parallel {
            request["parallel_tool_calls"] = json!(parallel);
        }
        let calls: Vec<_> = (0..3)
            .map(|index| {
                let mut item = call("inspect");
                item["id"] = json!(format!("fc_batch_{index}"));
                item["call_id"] = json!(format!("call_batch_{index}"));
                item
            })
            .collect();
        let mut translator = Translator::new(request);
        translator.push(&frame(completed(json!(calls))));
        let body = output(&mut translator);
        if parallel == Some(false) {
            assert!(body.contains("event: error"));
            assert!(body.contains("output.run_officejs.parallel_tool_calls"));
            assert!(!body.contains("response.function_call_arguments.delta"));
            continue;
        }
        let mut pending = body.into_bytes();
        let mut terminal = None;
        while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
            let event: Value = serde_json::from_slice(&crate::protocol::sse_data(&frame)).unwrap();
            if event["type"] == "response.completed" {
                terminal = Some(event["response"].clone());
            }
        }
        let terminal = terminal.unwrap();
        assert_eq!(terminal["output"].as_array().unwrap().len(), 3);
        for (index, item) in terminal["output"].as_array().unwrap().iter().enumerate() {
            assert_eq!(item["name"], "inspect");
            assert_eq!(item["call_id"], format!("call_batch_{index}"));
        }
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
fn local_failures_use_the_responses_error_event_contract() {
    let parameter = "output.run_officejs.args.json";
    let bytes = failure(
        crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID,
        Some(parameter),
    );
    let event: Value = serde_json::from_slice(&sse_event_data(&bytes)).unwrap();
    assert_eq!(event["type"], "error");
    assert_eq!(
        event["code"],
        crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID
    );
    assert_eq!(event["param"], parameter);
    assert!(event["message"].as_str().unwrap().contains(parameter));

    let mut translator = Translator::new(request());
    translator.push(&frame(json!({"type":"response.created","response":{"id":"synthetic","status":"in_progress","output":[]}})));
    let mut invalid = call("inspect");
    invalid["arguments"] = json!({"references":["inspect"],"code":"{invalid"})
        .to_string()
        .into();
    translator.push(&frame(completed(json!([invalid]))));
    let wire = output(&mut translator);
    let events: Vec<Value> = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["type"], "error");
    assert_eq!(
        events[1]["code"],
        crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID
    );
    assert_eq!(events[1]["sequence_number"], 1);
    assert!(translator.terminal);
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
    assert_eq!(
        parsed.error_category,
        Some(crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID)
    );
    let details = parsed.upstream_error.unwrap();
    assert_eq!(
        details.code.as_deref(),
        Some(crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID)
    );
    assert!(details
        .message
        .unwrap()
        .contains("parameter: output.run_officejs.args"));
}

#[test]
fn stream_preserves_literal_unknown_string_escapes() {
    let mut function = call("inspect");
    function["arguments"] = json!({
        "references":["inspect"],"code":r#"{"path":"C:\Users\Example\Work"}"#
    })
    .to_string()
    .into();
    let mut translator = Translator::new(request());
    translator.push(&frame(completed(json!([function]))));
    let body = output(&mut translator);
    let events: Vec<Value> = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert!(!events.iter().any(|event| event["type"] == "error"));
    let response = events
        .iter()
        .find(|event| event["type"] == "response.completed")
        .unwrap();
    let arguments: Value = serde_json::from_str(
        response["response"]["output"][0]["arguments"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(arguments, json!({"path":r#"C:\Users\Example\Work"#}));
}

#[test]
fn streamed_json_diagnostics_preserve_parser_reason_without_arguments() {
    let mut invalid = call("inspect");
    invalid["arguments"] = json!({
        "references":["inspect"],"code":r#"{"path":"synthetic-private\u12X4"}"#
    })
    .to_string()
    .into();
    let mut translator = Translator::new(request());
    translator.push(&frame(completed(json!([invalid]))));
    let body = output(&mut translator);
    assert!(body.contains("json_reason=invalid_escape"), "{body}");
    assert!(body.contains("payload_shape=json_object"));
    assert!(body.contains("parse_line=1"));
    assert!(body.contains("parse_column="));
    assert!(!body.contains("synthetic-private"));
    assert!(!body.contains("function_call_arguments"));
    assert!(!body.contains("response.completed"));
    let parsed = super::super::parse_sse_event(body.as_bytes());
    let message = parsed.upstream_error.unwrap().message.unwrap();
    assert!(message.contains("json_reason=invalid_escape"));
    assert!(message.contains("payload_shape=json_object"));
    assert!(translator.terminal);
}

#[test]
fn invalid_json_arguments_keep_their_cause_without_releasing_a_valid_subset() {
    for (code, parameter, cause) in [
        (
            r#"{"path":"private-value""#,
            "output.run_officejs.args.json.eof",
            "ended before",
        ),
        (
            r#"["private-value"]"#,
            "output.run_officejs.args.object",
            "one complete JSON object",
        ),
        (
            r#"{"path":"private-value",}"#,
            "output.run_officejs.args.json.syntax",
            "invalid syntax",
        ),
    ] {
        let mut translator = Translator::new(request());
        let mut invalid = call("inspect");
        invalid["id"] = json!("fc_invalid");
        invalid["call_id"] = json!("call_invalid");
        invalid["arguments"] = json!({"references":["inspect"],"code":code})
            .to_string()
            .into();
        translator.push(&frame(completed(json!([call("inspect"), invalid]))));
        let body = output(&mut translator);
        let parsed = super::super::parse_sse_event(body.as_bytes());
        assert_eq!(
            parsed.error_category,
            Some(crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID)
        );
        assert!(body.contains("one complete JSON object"));
        assert!(body.contains(parameter));
        assert!(body.contains(cause));
        let details = parsed.upstream_error.unwrap();
        let message = details.message.unwrap();
        assert!(message.contains(parameter));
        assert!(message.contains(cause));
        assert!(!body.contains("private-value"));
        assert!(!body.contains("response.function_call_arguments"));
        assert!(!body.contains("response.completed"));
        assert!(translator.terminal);
    }
}

#[test]
fn stream_accepts_referenced_compatibility_calls_only_after_completion() {
    for code in [
        r#"return await inspect({"path":"synthetic"})"#.to_string(),
        json!({"tool":"inspect","args":{"path":"synthetic"}}).to_string(),
        json!({"name":"inspect","arguments":r#"{"path":"synthetic"}"#}).to_string(),
    ] {
        let mut function = call("inspect");
        function["arguments"] = json!({"references":["inspect"],"code":code})
            .to_string()
            .into();
        let mut translator = Translator::new(request());
        translator.push(&frame(
            json!({"type":"response.output_item.done","item":function}),
        ));
        assert!(!output(&mut translator).contains("function_call_arguments"));
        for chunk in frame(completed(json!([function]))).chunks(7) {
            translator.push(chunk);
        }
        let body = output(&mut translator);
        let events: Vec<Value> = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect();
        assert!(!events.iter().any(|event| event["type"] == "error"));
        let response = events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .unwrap();
        assert_eq!(response["response"]["output"][0]["name"], "inspect");
        assert_eq!(response["response"]["output"][0]["call_id"], "call_1");
        assert_eq!(
            response["response"]["output"][0]["arguments"],
            r#"{"path":"synthetic"}"#
        );
    }
}

#[test]
fn fragmented_stream_preserves_literal_line_breaks_in_function_json_strings() {
    let mut function = call("inspect");
    function["arguments"] = json!({
        "references":["inspect"],"code":"{\"path\":\"synthetic\r\n\t😀\"}"
    })
    .to_string()
    .into();
    let wire = frame(completed(json!([function])));
    for size in [1, 7, wire.len()] {
        let mut translator = Translator::new(request());
        for chunk in wire.chunks(size) {
            translator.push(chunk);
        }
        let body = output(&mut translator);
        let events: Vec<Value> = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str(data).unwrap())
            .collect();
        assert!(!events.iter().any(|event| event["type"] == "error"));
        let response = events
            .iter()
            .find(|event| event["type"] == "response.completed")
            .unwrap();
        let arguments: Value = serde_json::from_str(
            response["response"]["output"][0]["arguments"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(arguments, json!({"path":"synthetic\r\n\t😀"}));
        assert!(translator.terminal);
    }
}

#[test]
fn fragmented_stream_preserves_function_json_and_custom_raw_input() {
    let mut request = request();
    request["tools"].as_array_mut().unwrap().push(json!({
        "type":"custom", "name":"apply_patch", "format":{"type":"text"}
    }));
    let arguments = json!({"path":"C:\\synthetic\\quoted\"folder\\file-😀.txt\nline\t\""});
    let patch = "*** Begin Patch\n*** Add File: synthetic.txt\n+\"quoted\" \\ 😀\n*** End Patch\n";
    let mut function = call("inspect");
    let header = serde_json::to_string_pretty(&json!({"field":"path","args":{}}))
        .unwrap()
        .replace('\n', "\r\n");
    function["arguments"] = json!({"references":["inspect"],"code":format!("relay-function-string-v1 {header}\r\n{}", arguments["path"].as_str().unwrap())})
        .to_string()
        .into();
    let custom = json!({
        "type":"function_call", "id":"fc_patch", "call_id":"call_patch", "name":"run_officejs",
        "arguments":json!({"references":["apply_patch"],"code":patch}).to_string()
    });
    let wire = frame(completed(json!([function, custom])));
    for size in [1, 7, 64, wire.len()] {
        let mut translator = Translator::new(request.clone());
        for chunk in wire.chunks(size) {
            translator.push(chunk);
        }
        let body = output(&mut translator);
        let mut pending = body.into_bytes();
        let mut terminal = None;
        while let Some(frame) = take_sse_event(&mut pending) {
            let event: Value = serde_json::from_slice(&sse_event_data(&frame)).unwrap();
            if event["type"] == "response.completed" {
                terminal = Some(event["response"].clone());
            }
        }
        let terminal = terminal.expect("a valid tool response must complete");
        let actual: Value =
            serde_json::from_str(terminal["output"][0]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(actual, arguments);
        assert_eq!(terminal["output"][1]["type"], "custom_tool_call");
        assert_eq!(terminal["output"][1]["input"], patch);
    }
}

#[test]
fn schema_failure_reports_safe_cause_in_persisted_diagnostics() {
    let frame = failure(
        crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID,
        Some("output.run_officejs.args.schema"),
    );
    let parsed = super::super::parse_sse_event(&frame);
    assert_eq!(
        parsed.error_category,
        Some(crate::error_codes::ADAPTER_UPSTREAM_STREAM_INVALID)
    );
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
fn streamed_schema_diagnostics_withhold_tools_and_preserve_the_safe_constraint() {
    for (arguments, keyword) in [
        (json!({"path":42}), "type"),
        (
            json!({"path":"test","synthetic-private-extra":"synthetic-private-value"}),
            "additionalProperties",
        ),
    ] {
        let invalid = json!({
            "type":"function_call","call_id":"call_schema","name":"run_officejs",
            "arguments":json!({"references":["inspect"],"code":arguments.to_string()}).to_string()
        });
        let event = completed(json!([call("inspect"), invalid]));
        let response = &event["response"];
        let json_error = crate::gateway::execution::basis_points::translate_response(
            &serde_json::to_vec(response).unwrap(),
            &request(),
        )
        .unwrap_err();
        assert_eq!(
            json_error.parameter(),
            Some("output.run_officejs.args.schema")
        );
        assert!(json_error
            .to_string()
            .contains(&format!("schema_keyword={keyword}")));

        let wire = frame(event);
        for size in [1, 7, wire.len()] {
            let mut translator = Translator::new(request());
            for chunk in wire.chunks(size) {
                translator.push(chunk);
            }
            let body = output(&mut translator);
            assert!(body.contains("schema_phase=arguments"));
            assert!(body.contains(&format!("schema_keyword={keyword}")));
            assert!(!body.contains("response.function_call_arguments.done"));
            assert!(!body.contains("response.output_item.added"));
            assert!(!body.contains("response.completed"));
            assert!(!body.contains("synthetic-private"));
            assert!(translator.terminal);
            let parsed = super::super::parse_sse_event(body.as_bytes());
            assert!(parsed
                .upstream_error
                .unwrap()
                .message
                .unwrap()
                .contains(&format!("schema_keyword={keyword}")));
        }
    }
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
            .event(json!({"type":"response.function_call_arguments.delta","item_id":format!("fc_{index}"),"delta":"{"}))
            .unwrap();
    }
    assert!(translator.event(json!({"type":"response.function_call_arguments.delta","item_id":"overflow","delta":"{"})).is_err());
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
