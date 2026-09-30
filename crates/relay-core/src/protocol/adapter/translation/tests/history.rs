use super::*;
use serde_json::json;

#[test]
fn responses_continuations_replace_instructions_and_preserve_system_history() {
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let first = prepare(
            WireApi::Responses,
            upstream,
            &json!({"input":[
            {"role":"system","content":"Permanent policy"},
            {"role":"user","content":"Hello"}
        ],"instructions":"Temporary first instruction"}),
            false,
        );
        let completed = first
            .translate_response_bytes(&serde_json::to_vec(&output(upstream)).unwrap())
            .unwrap()
            .unwrap();
        let (_, previous) = completed.continuation().unwrap();
        for instruction in [Some("New instruction"), None] {
            let mut request =
                json!({"input":"Continue","previous_response_id":completed.response_id()});
            if let Some(instruction) = instruction {
                request["instructions"] = instruction.into();
            }
            let continued = SourceAdapter::between(WireApi::Responses, upstream)
                .unwrap()
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Responses,
                    request: &request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: Some(previous.clone()),
                    response_scope: "source-test",
                    response_id_seed: "second",
                })
                .unwrap();
            let body = continued.upstream_body().to_string();
            assert!(body.contains("Permanent policy"), "{upstream:?}");
            assert!(
                !body.contains("Temporary first instruction"),
                "{upstream:?}"
            );
            assert_eq!(body.contains("New instruction"), instruction.is_some());
        }
    }
}
#[test]
fn translated_continuation_preserves_reasoning_mode() {
    let adapter = SourceAdapter::ResponsesToChatCompletions;
    let first_request = json!({"input":"Hello"});
    let first = adapter
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &first_request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "source-test",
            response_id_seed: "first",
        })
        .unwrap();
    let completed = first
        .translate_response_bytes(&serde_json::to_vec(&output(WireApi::ChatCompletions)).unwrap())
        .unwrap()
        .unwrap();
    let (response_id, previous) = completed.continuation().unwrap();
    assert_eq!(previous.reasoning_mode, MessagesReasoningMode::Disabled);

    let continued_request = json!({
        "input":"Continue",
        "previous_response_id": response_id
    });
    let continued = adapter
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &continued_request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: Some(previous.clone()),
            response_scope: "source-test",
            response_id_seed: "second",
        })
        .unwrap();
    let messages = continued.upstream_body()["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0]["content"][0]["text"], "Hello");
    assert_eq!(messages[2]["content"][0]["text"], "Continue");

    let mismatch = adapter.prepare_request(AdapterRequestContext {
        client_wire_api: WireApi::Responses,
        request: &continued_request,
        model: "test",
        stream: false,
        reasoning_mode: MessagesReasoningMode::Adaptive,
        cache_write_ttl: CacheWriteTtl::Provider,
        previous: Some(previous.clone()),
        response_scope: "source-test",
        response_id_seed: "second",
    });
    assert_eq!(
        mismatch.unwrap_err().code(),
        "adapter_continuation_mismatch"
    );
}
#[test]
fn converted_storage_requests_are_rejected_before_sending() {
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        for upstream in WireApi::ALL {
            let mut request = input(client);
            request["store"] = true.into();
            let adapter = SourceAdapter::between(client, upstream).unwrap();
            let result = adapter.prepare_request(AdapterRequestContext {
                client_wire_api: client,
                request: &request,
                model: "test",
                stream: false,
                reasoning_mode: if client == upstream {
                    MessagesReasoningMode::Disabled
                } else {
                    MessagesReasoningMode::Adaptive
                },
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "test",
                response_id_seed: "test",
            });
            if client == upstream {
                assert_eq!(result.unwrap().upstream_body()["store"], true);
            } else {
                assert_eq!(result.unwrap_err().code(), "adapter_parameter_unsupported");
            }
        }
    }
}
#[test]
fn reasoning_preserves_schema_and_honors_output_limits() {
    let request = json!({"input":"Hello","max_output_tokens":64,"reasoning":{"effort":"high"},
        "text":{"format":{"type":"json_schema","name":"answer","schema":{"type":"object","properties":{"ok":{"type":"boolean"}}}}}});
    for upstream in [WireApi::Messages, WireApi::Gemini] {
        let prepared = prepare(WireApi::Responses, upstream, &request, false);
        if upstream == WireApi::Messages {
            assert_eq!(
                prepared.upstream_body()["output_config"]["format"]["schema"],
                request["text"]["format"]["schema"]
            );
            assert_eq!(prepared.upstream_body()["output_config"]["effort"], "high");
            assert_eq!(prepared.upstream_body()["max_tokens"], 64);
        } else {
            assert_eq!(
                prepared.upstream_body()["generationConfig"]["thinkingConfig"]["thinkingLevel"],
                "high"
            );
            assert_eq!(
                prepared.upstream_body()["generationConfig"]["maxOutputTokens"],
                64
            );
        }
    }
    let invalid = json!({"messages":[{"role":"user","content":"Hello"}],"thinking":{"type":"enabled","budget_tokens":2048},"max_tokens":100});
    let parsed = decode::request(WireApi::Messages, &invalid).unwrap();
    assert!(encode::request(&parsed, WireApi::Messages, "test", false).is_err());
}
#[test]
fn messages_thinking_blocks_are_translated_as_reasoning() {
    let response = response::decode(
        WireApi::Messages,
        &json!({
            "id":"msg_thinking",
            "stop_reason":"end_turn",
            "content":[
                {"type":"thinking","thinking":"plan first"},
                {"type":"text","text":"answer"}
            ],
            "usage":{"input_tokens":4,"output_tokens":3}
        }),
        "seed",
    )
    .unwrap();
    assert!(matches!(&response.blocks[0], Block::Reasoning(text) if text == "plan first"));
    assert!(matches!(&response.blocks[1], Block::Text(text) if text == "answer"));

    let encoded =
        response::encode(WireApi::Messages, &response, "test", &Default::default()).unwrap();
    assert_eq!(encoded["content"][0]["type"], "thinking");
    assert_eq!(encoded["content"][0]["thinking"], "plan first");
}
#[test]
fn messages_stream_thinking_blocks_complete_without_dropping_reasoning() {
    let mut bridge = prepare(
        WireApi::Responses,
        WireApi::Messages,
        &json!({"input":"Hello"}),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    for event in [
        json!({"type":"message_start","message":{"id":"msg_thinking","usage":{"input_tokens":4}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"plan "}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"first"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":"answer"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}),
        json!({"type":"message_stop"}),
    ] {
        bridge.push(format!("data: {event}\n\n").as_bytes());
    }
    bridge.finish();
    let completed = bridge.completed().expect("thinking stream should complete");
    let response = response::decode(WireApi::Responses, &completed.response_body, "test").unwrap();
    assert!(matches!(&response.blocks[0], Block::Text(text) if text == "answer"));
    assert_eq!(
        completed.continuation.messages[1]["content"][0]["thinking"],
        "plan first"
    );
}
#[test]
fn responses_to_chat_continuation_resolves_results_after_loading_history() {
    let mut previous = MessagesBridgeState::new("test", MessagesReasoningMode::Adaptive);
    previous.portable_history = Some(vec![Message {
        role: Role::Assistant,
        blocks: vec![Block::ToolCall {
            id: "call_saved".into(),
            name: "lookup".into(),
            arguments: "{}".into(),
        }],
    }]);
    let request = json!({"model":"test","previous_response_id":"resp_saved","input":[
        {"type":"function_call_output","call_id":"call_saved","output":"found"}
    ]});
    let prepared = SourceAdapter::ResponsesToChatCompletions
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: Some(previous),
            response_scope: "source",
            response_id_seed: "next",
        })
        .unwrap();
    assert_eq!(
        prepared.upstream_body()["messages"][1]["tool_call_id"],
        "call_saved"
    );
    assert_eq!(prepared.upstream_body()["messages"][1]["content"], "found");
}
