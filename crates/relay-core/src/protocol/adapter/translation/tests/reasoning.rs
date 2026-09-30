use super::*;

#[test]
fn chat_responses_preserve_portable_reasoning_and_tools_from_each_protocol() {
    for (upstream, body) in [
        (
            WireApi::Responses,
            json!({"status":"completed","output":[
            {"type":"reasoning","summary":[{"type":"summary_text","text":"Check first"}]},
            {"type":"function_call","call_id":"call_test","name":"lookup","arguments":"{}"}
        ],"usage":{"input_tokens":3,"output_tokens":2}}),
        ),
        (
            WireApi::Messages,
            json!({"stop_reason":"tool_use","content":[
            {"type":"thinking","thinking":"Check first"},
            {"type":"tool_use","id":"call_test","name":"lookup","input":{}}
        ],"usage":{"input_tokens":3,"output_tokens":2}}),
        ),
        (
            WireApi::Gemini,
            json!({"candidates":[{"content":{"parts":[
            {"text":"Check first","thought":true},
            {"functionCall":{"id":"call_test","name":"lookup","args":{}}}
        ]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}}),
        ),
    ] {
        let translated = prepare(
            WireApi::ChatCompletions,
            upstream,
            &input(WireApi::ChatCompletions),
            false,
        )
        .translate_response_bytes(&serde_json::to_vec(&body).unwrap())
        .unwrap()
        .unwrap();
        let result = translated.response_body();
        let message = &result["choices"][0]["message"];
        assert_eq!(message["reasoning_content"], "Check first", "{upstream:?}");
        assert_eq!(message["tool_calls"][0]["id"], "call_test");
        assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(result["usage"]["prompt_tokens"], 3);
        assert_eq!(result["usage"]["completion_tokens"], 2);
        assert!(result["usage"].get("total_tokens").is_none());

        let history = json!({"messages":[{"role":"user","content":"Check"},message,
            {"role":"tool","tool_call_id":"call_test","content":"found"}]});
        for destination in [WireApi::Messages, WireApi::Gemini] {
            let prepared = prepare(WireApi::ChatCompletions, destination, &history, false);
            let parsed = decode::request(destination, prepared.upstream_body()).unwrap();
            let blocks = &parsed.messages[1].blocks;
            assert!(matches!(&blocks[0], Block::Reasoning(text) if text == "Check first"));
            assert!(matches!(&blocks[1], Block::ToolCall {id, ..} if id == "call_test"));
        }
    }
}

#[test]
fn chat_stream_keeps_reasoning_deltas_tool_indices_and_usage() {
    let mut bridge = prepare(
        WireApi::ChatCompletions,
        WireApi::Messages,
        &input(WireApi::ChatCompletions),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    let events = [
        json!({"type":"message_start","message":{"id":"msg_test","usage":{"input_tokens":3}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"Check "}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"first"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_test","name":"lookup","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
        json!({"type":"message_stop"}),
    ];
    for event in events {
        for fragment in format!("data: {event}\n\n").as_bytes().chunks(7) {
            bridge.push(fragment);
        }
    }
    bridge.finish();
    let completed = bridge
        .completed()
        .expect("portable reasoning stream must complete");
    assert_eq!(
        completed.response_body["choices"][0]["message"]["reasoning_content"],
        "Check first"
    );
    let mut chunks = Vec::new();
    while let Some(chunk) = bridge.pop_output() {
        chunks.extend(chunk);
    }
    let stream = String::from_utf8(chunks).unwrap();
    assert!(stream.ends_with("data: [DONE]\n\n"));
    let values = stream
        .split("\n\n")
        .filter_map(|event| event.strip_prefix("data: "))
        .filter(|event| *event != "[DONE]")
        .map(|event| serde_json::from_str::<Value>(event).unwrap())
        .collect::<Vec<_>>();
    let reasoning = values
        .iter()
        .filter_map(|event| {
            event
                .pointer("/choices/0/delta/reasoning_content")
                .and_then(Value::as_str)
        })
        .collect::<String>();
    assert_eq!(reasoning, "Check first");
    let call = values
        .iter()
        .find_map(|event| event.pointer("/choices/0/delta/tool_calls/0"))
        .unwrap();
    assert_eq!(call["index"], 0);
    assert_eq!(call["id"], "call_test");
    assert_eq!(values.last().unwrap()["usage"]["completion_tokens"], 2);
}

#[test]
fn public_summaries_replay_to_chat_without_accepting_encrypted_state() {
    let mut request = json!({"input":[{"type":"reasoning","id":"rs_test",
        "summary":[{"type":"summary_text","text":"Check first"}]},
        {"role":"user","content":"Continue"}]});
    let prepared = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &request,
        false,
    );
    assert_eq!(
        prepared.upstream_body()["messages"][0]["reasoning_content"],
        "Check first"
    );
    request["input"][0]["encrypted_content"] = "synthetic opaque state".into();
    assert!(decode::request(WireApi::Responses, &request).is_err());
    for role in ["user", "system", "tool"] {
        assert!(decode::request(WireApi::ChatCompletions,
            &json!({"messages":[{"role":role,"content":"text","reasoning_content":"invalid owner"}]})).is_err());
    }
}

#[test]
fn responses_continuation_replays_chat_reasoning_with_the_tool_result() {
    let previous_request = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &input(WireApi::Responses),
        false,
    );
    let completed = previous_request.translate_response_bytes(&serde_json::to_vec(&json!({
        "id":"chat_test","choices":[{"finish_reason":"tool_calls","message":{
            "role":"assistant","content":null,"reasoning_content":"Check first",
            "tool_calls":[{"id":"call_test","type":"function","function":{"name":"lookup","arguments":"{}"}}]
        }}]
    })).unwrap()).unwrap().unwrap();
    let request = json!({"input":[{"type":"function_call_output","call_id":"call_test","output":"found"}],
        "previous_response_id":completed.response_id()});
    let (_, previous) = completed.continuation().unwrap();
    let continued = SourceAdapter::ResponsesToChatCompletions
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: Some(previous.clone()),
            response_scope: "source-test",
            response_id_seed: "next",
        })
        .unwrap();
    let messages = &continued.upstream_body()["messages"];
    assert_eq!(messages[1]["reasoning_content"], "Check first");
    assert_eq!(messages[1]["tool_calls"][0]["id"], "call_test");
    assert_eq!(messages[2]["tool_call_id"], "call_test");
    assert_eq!(messages[2]["content"], "found");
}
