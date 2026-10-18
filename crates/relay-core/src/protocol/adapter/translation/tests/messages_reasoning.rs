use super::*;

fn request() -> Value {
    json!({"messages":[{"role":"user","content":"Update the synthetic file."}],
        "max_tokens":4096,"thinking":{"type":"adaptive"},"output_config":{"effort":"high"},
        "tools":[{"name":"edit_file","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}]})
}

fn reply() -> Value {
    json!({"id":"resp_synthetic","object":"response","status":"completed","output":[
        {"type":"reasoning","id":"rs_synthetic","summary":[{"type":"summary_text","text":"Plan the edit."}],"encrypted_content":"synthetic-opaque-state"},
        {"type":"function_call","id":"fc_synthetic","call_id":"call_edit","name":"edit_file","arguments":"{\"path\":\"synthetic.txt\"}","status":"completed"}
    ],"usage":{"input_tokens":10,"output_tokens":8,"output_tokens_details":{"reasoning_tokens":5}}})
}

fn assert_reasoning_tool_roundtrip(completed: &TranslatedResponse, native: &Value) {
    let body = &completed.response_body;
    let content = body["content"].as_array().unwrap();
    if native["output"][0]["summary"] == json!([]) {
        assert_eq!(content.len(), 1);
    } else {
        assert_eq!(content.len(), 2);
        assert_eq!(
            content[0],
            json!({"type":"thinking","thinking":"Plan the edit."})
        );
    }
    let tool = content.last().unwrap();
    assert_eq!(tool["type"], "tool_use");
    assert_eq!(tool["id"], "call_edit");
    assert_eq!(tool["name"], "edit_file");
    assert_eq!(tool["input"], json!({"path":"synthetic.txt"}));
    assert_eq!(body["stop_reason"], "tool_use");
    assert_eq!(body["usage"]["input_tokens"], 10);
    assert_eq!(body["usage"]["output_tokens"], 8);
    // A harness may retain the public summary or omit it from portable history.
    // Neither choice may discard the original provider state after a tool call.
    for include_summary in [true, false] {
        let assistant = content
            .iter()
            .filter(|block| include_summary || block["type"] != "thinking")
            .cloned()
            .collect::<Vec<_>>();
        let mut next = request();
        next["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":assistant}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_edit","content":"updated"}]}),
        ]);
        let continued = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &next,
                model: "test",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: Some(completed.continuation.clone()),
                response_scope: "source-test",
                response_id_seed: "next",
            })
            .unwrap();
        let history = continued.upstream_body()["input"].as_array().unwrap();
        assert_eq!(history.len(), 4);
        assert_eq!(history[1], native["output"][0]);
        assert_eq!(history[2], native["output"][1]);
        assert_eq!(history[3]["type"], "function_call_output");
        assert_eq!(history[3]["call_id"], "call_edit");
        assert_eq!(history[3]["output"], "updated");
        assert!(!continued.upstream_body().to_string().contains("signature"));
        let finished = continued
            .translate_response_bytes(&serde_json::to_vec(&output(WireApi::Responses)).unwrap())
            .unwrap()
            .unwrap();
        let answer = &finished.translated_response().response_body;
        assert_eq!(answer["content"], json!([{"type":"text","text":"Hi"}]));
        assert_eq!(answer["stop_reason"], "end_turn");
    }
}

#[test]
fn messages_json_keeps_public_reasoning_and_edit_tool_with_native_replay() {
    let response = prepare(WireApi::Messages, WireApi::Responses, &request(), false)
        .translate_response_bytes(&serde_json::to_vec(&reply()).unwrap())
        .unwrap()
        .unwrap();
    assert_reasoning_tool_roundtrip(response.translated_response(), &reply());
}

#[test]
fn messages_harness_empty_signature_keeps_native_tool_continuation() {
    let completed = prepare(WireApi::Messages, WireApi::Responses, &request(), false)
        .translate_response_bytes(&serde_json::to_vec(&reply()).unwrap())
        .unwrap()
        .unwrap();
    let completed = completed.translated_response();
    for signature in [
        json!(""),
        Value::Null,
        json!("foreign-signature"),
        json!(false),
    ] {
        let mut assistant = completed.response_body["content"].clone();
        // Claude's stream assembler initializes unsigned thinking with an
        // empty signature, even when no signature_delta was received.
        assistant[0]["signature"] = signature.clone();
        let mut next = request();
        next["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":assistant}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_edit","content":"updated"}]}),
        ]);
        let native = prepare(WireApi::Messages, WireApi::Messages, &next, false);
        assert_eq!(native.upstream_body()["messages"], next["messages"]);
        let key = continuation::request_key(WireApi::Messages, &next, "test");
        if signature == json!("") || signature.is_null() {
            assert_eq!(key.unwrap(), completed.continuation.history_key);
        } else {
            assert!(key.is_err());
        }
        let prepared = SourceAdapter::MessagesToResponses.prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Messages,
            request: &next,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: Some(completed.continuation.clone()),
            response_scope: "source-test",
            response_id_seed: "empty-signature",
        });
        if signature == json!("") || signature.is_null() {
            let prepared = prepared.expect("empty signature is not foreign opaque state");
            assert_eq!(prepared.upstream_body()["input"][1], reply()["output"][0]);
            assert_eq!(prepared.upstream_body()["input"][3]["call_id"], "call_edit");
            assert!(!prepared.upstream_body().to_string().contains("signature"));
            let missing = SourceAdapter::MessagesToResponses
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Messages,
                    request: &next,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "source-test",
                    response_id_seed: "missing-state",
                })
                .unwrap_err();
            assert_eq!(
                missing.code(),
                crate::error_codes::ADAPTER_CONTINUATION_MISSING
            );
        } else {
            let error = prepared.expect_err("foreign or malformed signatures must not be dropped");
            assert_eq!(error.parameter(), Some("messages[].content[].signature"));
        }
    }
}

fn response_events(response: &Value) -> Vec<Value> {
    let mut events = vec![
        json!({"type":"response.created","response":{"id":"resp_synthetic","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_synthetic","summary":[]}}),
    ];
    if response["output"][0]["summary"] != json!([]) {
        events.extend([
            json!({"type":"response.reasoning_summary_part.added","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"Plan the edit."}),
            json!({"type":"response.reasoning_summary_text.done","output_index":0,"summary_index":0,"text":"Plan the edit."}),
            json!({"type":"response.reasoning_summary_part.done","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":"Plan the edit."}}),
        ]);
    }
    events.extend([
        json!({"type":"response.output_item.done","output_index":0,"item":response["output"][0]}),
        json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_synthetic","call_id":"call_edit","name":"edit_file","arguments":"","status":"in_progress"}}),
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"path\":"}),
        json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"\"synthetic.txt\"}"}),
        json!({"type":"response.function_call_arguments.done","output_index":1,"arguments":"{\"path\":\"synthetic.txt\"}"}),
        json!({"type":"response.output_item.done","output_index":1,"item":response["output"][1]}),
        json!({"type":"response.completed","response":response}),
    ]);
    events
}

fn stream_response(response: &Value) -> (crate::AdapterStreamBridge, Vec<Value>) {
    let mut stream = prepare(WireApi::Messages, WireApi::Responses, &request(), true)
        .into_stream_bridge()
        .unwrap();
    for event in response_events(response) {
        stream.push(format!("data: {event}\n\n").as_bytes());
    }
    stream.finish();
    let frames = String::from_utf8(
        std::iter::from_fn(|| stream.pop_output())
            .flatten()
            .collect(),
    )
    .unwrap();
    assert!(!frames.contains("event: error"), "{frames}");
    assert!(frames.contains("input_json_delta"));
    assert!(!frames.contains("signature_delta"));
    assert!(!frames.contains("synthetic-opaque-state"));
    let events = frames
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    (stream, events)
}

#[test]
fn messages_stream_keeps_public_reasoning_and_edit_tool_with_native_replay() {
    let (stream, events) = stream_response(&reply());
    let kinds = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        kinds,
        vec![
            "message_start",
            "content_block_start",
            "content_block_delta",
            "content_block_stop",
            "content_block_start",
            "content_block_delta",
            "content_block_delta",
            "content_block_stop",
            "message_delta",
            "message_stop",
        ]
    );
    assert_eq!(
        events[1]["content_block"],
        json!({"type":"thinking","thinking":""})
    );
    assert_eq!(
        events[2]["delta"],
        json!({"type":"thinking_delta","thinking":"Plan the edit."})
    );
    assert_eq!(events[4]["index"], 1);
    assert_eq!(events[4]["content_block"]["id"], "call_edit");
    assert_eq!(events[8]["delta"]["stop_reason"], "tool_use");
    assert_reasoning_tool_roundtrip(
        stream
            .completed()
            .expect("reasoning must not abort the edit"),
        &reply(),
    );
}

#[test]
fn messages_json_and_stream_replay_opaque_reasoning_without_public_summary() {
    let mut native = reply();
    native["output"][0]["summary"] = json!([]);
    let response = prepare(WireApi::Messages, WireApi::Responses, &request(), false)
        .translate_response_bytes(&serde_json::to_vec(&native).unwrap())
        .unwrap()
        .unwrap();
    assert_reasoning_tool_roundtrip(response.translated_response(), &native);
    let (stream, events) = stream_response(&native);
    assert!(events.iter().all(|event| {
        event["content_block"]["type"] != "thinking" && event["delta"]["type"] != "thinking_delta"
    }));
    assert_reasoning_tool_roundtrip(stream.completed().unwrap(), &native);
}

#[test]
fn messages_json_preserves_public_reasoning_and_tools_from_chat_and_gemini() {
    for (upstream, native) in [
        (
            WireApi::ChatCompletions,
            json!({"id":"chat_synthetic","choices":[{"index":0,"message":{"role":"assistant","reasoning_content":"Plan the edit.","content":null,"tool_calls":[{"id":"call_edit","type":"function","function":{"name":"edit_file","arguments":"{\"path\":\"synthetic.txt\"}"}}]},"finish_reason":"tool_calls"}]}),
        ),
        (
            WireApi::Gemini,
            json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Plan the edit.","thought":true},{"functionCall":{"id":"call_edit","name":"edit_file","args":{"path":"synthetic.txt"}},"thoughtSignature":"synthetic-signature"}]},"finishReason":"STOP"}]}),
        ),
    ] {
        let response = prepare(WireApi::Messages, upstream, &request(), false)
            .translate_response_bytes(&serde_json::to_vec(&native).unwrap())
            .unwrap()
            .unwrap();
        let body = &response.translated_response().response_body;
        assert_eq!(
            body["content"][0],
            json!({"type":"thinking","thinking":"Plan the edit."})
        );
        assert_eq!(
            body["content"][1],
            json!({"type":"tool_use","id":"call_edit","name":"edit_file","input":{"path":"synthetic.txt"}})
        );
        assert_eq!(body["stop_reason"], "tool_use");
    }
}
