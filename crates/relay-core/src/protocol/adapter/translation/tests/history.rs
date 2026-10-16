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
    let mut foreign_client = previous.clone();
    foreign_client.client_wire_api = Some(WireApi::ChatCompletions);
    let mismatch = adapter.prepare_request(AdapterRequestContext {
        client_wire_api: WireApi::Responses,
        request: &continued_request,
        model: "test",
        stream: false,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: CacheWriteTtl::Provider,
        previous: Some(foreign_client),
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
    assert!(encode::request(
        &parsed,
        WireApi::Messages,
        "test",
        false,
        &Default::default(),
        MessagesReasoningMode::Adaptive,
        None,
    )
    .is_err());
}
#[test]
fn messages_thinking_blocks_are_translated_as_reasoning() {
    let response = response::decode(
        WireApi::Messages,
        &json!({
            "id":"msg_thinking",
            "stop_reason":"end_turn",
            "content":[
                {"type":"thinking","thinking":"plan first","signature":"synthetic-signature"},
                {"type":"text","text":"answer"}
            ],
            "usage":{"input_tokens":4,"output_tokens":3}
        }),
        "seed",
    )
    .unwrap();
    assert!(matches!(&response.blocks[0], Block::Reasoning(text) if text == "plan first"));
    assert!(matches!(&response.blocks[1], Block::Text(text) if text == "answer"));

    let encoded = response::encode(
        WireApi::ChatCompletions,
        &response,
        "test",
        &Default::default(),
    )
    .unwrap();
    assert_eq!(
        encoded["choices"][0]["message"]["reasoning_content"],
        "plan first"
    );
    assert_eq!(
        response.native_output[0]["signature"],
        "synthetic-signature"
    );
    let portable =
        response::encode(WireApi::Messages, &response, "test", &Default::default()).unwrap();
    assert_eq!(
        portable["content"][0],
        json!({"type":"thinking","thinking":"plan first"})
    );
    assert_eq!(
        response.native_output[0]["signature"],
        "synthetic-signature"
    );
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
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"synthetic-signature"}}),
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
    assert!(matches!(&response.blocks[0], Block::Reasoning(text) if text == "plan first"));
    assert!(matches!(&response.blocks[1], Block::Text(text) if text == "answer"));
    assert_eq!(
        completed.continuation.native_turns[&1][0]["thinking"],
        "plan first"
    );
    assert_eq!(
        completed.continuation.native_turns[&1][0]["signature"],
        "synthetic-signature"
    );
}

#[test]
fn messages_stream_rejects_thinking_without_a_signature() {
    let mut bridge = prepare(
        WireApi::Responses,
        WireApi::Messages,
        &json!({"input":"Hello"}),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    for event in [
        json!({"type":"message_start","message":{"id":"msg_thinking"}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"plan"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        json!({"type":"message_stop"}),
    ] {
        bridge.push(format!("data: {event}\n\n").as_bytes());
    }
    bridge.finish();
    assert!(bridge.is_terminal());
    assert!(bridge.completed().is_none());
    let output = std::iter::from_fn(|| bridge.pop_output())
        .flatten()
        .collect::<Vec<_>>();
    assert!(String::from_utf8(output)
        .unwrap()
        .contains("adapter_upstream_stream_invalid"));
}
#[test]
fn responses_to_chat_continuation_resolves_results_after_loading_history() {
    let mut previous = MessagesBridgeState::new("test", MessagesReasoningMode::Adaptive);
    previous.client_wire_api = Some(WireApi::Responses);
    previous.response_call_targets.insert(
        "call_saved".into(),
        ClientToolTarget {
            kind: ResponsesToolKind::Function,
            name: "lookup".into(),
            namespace: None,
        },
    );
    previous.upstream_protocol = Some(WireApi::ChatCompletions);
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

#[test]
fn unknown_or_reasoning_models_require_native_state_for_tool_result_history() {
    let request = tool_history(WireApi::ChatCompletions);
    for upstream in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
        for reasoning in [None, Some(true)] {
            let capabilities = crate::model_metadata::ModelCapabilities {
                reasoning,
                ..Default::default()
            };
            let error = TranslationRequest::prepare(
                AdapterRequestContext {
                    client_wire_api: WireApi::ChatCompletions,
                    request: &request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "source-test",
                    response_id_seed: "missing-native-state",
                },
                upstream,
                Some(&capabilities),
            )
            .unwrap_err();
            assert_eq!(
                error.code(),
                "adapter_continuation_missing",
                "{upstream:?}: {reasoning:?}"
            );
        }
    }
}

#[test]
fn encrypted_responses_reasoning_without_public_summary_is_preserved_for_replay() {
    let response = response::decode(WireApi::Responses, &json!({
        "status":"completed","output":[
            {"type":"reasoning","id":"rs_synthetic","encrypted_content":"synthetic-encrypted-state"},
            {"type":"message","role":"assistant","content":[{"type":"output_text","text":"answer","annotations":[]}]}
        ]
    }), "seed").unwrap();
    assert_eq!(response.blocks, vec![Block::Text("answer".into())]);
    assert_eq!(
        response.native_output[0]["encrypted_content"],
        "synthetic-encrypted-state"
    );
    let completed = TranslationRequest::prepare(
        AdapterRequestContext {
            client_wire_api: WireApi::ChatCompletions,
            request: &input(WireApi::ChatCompletions),
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "source-test",
            response_id_seed: "encrypted-state",
        },
        WireApi::Responses,
        None,
    )
    .unwrap()
    .complete(response)
    .unwrap();
    let request = json!({"messages":[
        {"role":"user","content":"Hello"},{"role":"assistant","content":"answer"},{"role":"user","content":"Continue"}
    ]});
    let replay = TranslationRequest::prepare(
        AdapterRequestContext {
            client_wire_api: WireApi::ChatCompletions,
            request: &request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: Some(completed.continuation),
            response_scope: "source-test",
            response_id_seed: "replayed-state",
        },
        WireApi::Responses,
        None,
    )
    .unwrap();
    assert!(replay.upstream_body["input"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item["encrypted_content"] == "synthetic-encrypted-state"));
}

#[test]
fn bridge_preserves_public_summary_only_after_a_new_user_turn() {
    let mut request = json!({"input":[
        {"role":"user","content":"Hello"},
        {"type":"reasoning","id":"rs_cross_provider","encrypted_content":"opaque-ciphertext","summary":[
            {"type":"summary_text","text":"Thought about the greeting"}
        ]},
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hi","annotations":[]}]}
    ]});
    let adapter = SourceAdapter::between(WireApi::Responses, WireApi::ChatCompletions).unwrap();
    let error = adapter
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "source-test",
            response_id_seed: "request-test",
        })
        .unwrap_err();
    assert_eq!(error.code(), "adapter_parameter_unsupported");

    request["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"user","content":"Continue"}));
    let prepared = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &request,
        false,
    );
    let messages = prepared.upstream_body()["messages"].as_array().unwrap();
    assert_eq!(
        messages[0],
        json!({"role":"user","content":[{"type":"text","text":"Hello"}]})
    );
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(
        messages[1]["reasoning_content"],
        "Thought about the greeting"
    );
    assert_eq!(messages[1]["content"], json!([{"type":"text","text":"Hi"}]));
    assert_eq!(
        messages[2],
        json!({"role":"user","content":[{"type":"text","text":"Continue"}]})
    );
    assert_eq!(messages.len(), 3);
    let body = serde_json::to_string(prepared.upstream_body()).unwrap();
    assert!(!body.contains("opaque-ciphertext"));
}

#[test]
fn bridge_rejects_non_translatable_history_while_native_preserves_it() {
    let specialized_items = json!([
        {"type":"web_search_call","id":"ws_1","status":"completed"},
        {"type":"computer_call","id":"cc_1","status":"completed","call_id":"cc_1","action":{"type":"screenshot"}},
        {"type":"computer_call_output","call_id":"cc_1","output":"screen-captured"},
        {"type":"mcp_tool_call","id":"mcp_1","call_id":"mcp_1","name":"lookup","arguments":"{}"},
        {"type":"mcp_tool_call_output","call_id":"mcp_1","output":"found"},
        {"type":"file_search_call","id":"fs_1","status":"completed"},
        {"type":"image_generation_call","id":"ig_1","status":"completed","result":"aW1hZ2U="},
        {"type":"tool_call","id":"tc_1","call_id":"tc_1","name":"run","arguments":"{}"},
        {"type":"tool_call_output","call_id":"tc_1","output":"done"},
        {"type":"local_shell_call","call_id":"sh_1","action":{"type":"exec","command":["echo","test"]}},
        {"type":"local_shell_call_output","call_id":"sh_1","output":"test"}
    ]);
    for item in specialized_items.as_array().unwrap() {
        for stream in [false, true] {
            let request = json!({"input":[
                {"role":"user","content":"Search and compute"},
                item,
                {"role":"user","content":"Continue"}
            ]});
            let native = prepare(WireApi::Responses, WireApi::Responses, &request, stream);
            assert_eq!(native.upstream_body()["input"], request["input"]);
            let error = SourceAdapter::between(WireApi::Responses, WireApi::ChatCompletions)
                .unwrap()
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Responses,
                    request: &request,
                    model: "test",
                    stream,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "source-test",
                    response_id_seed: "request-test",
                })
                .unwrap_err();
            assert_eq!(error.code(), "adapter_parameter_unsupported", "{item}");
            assert_eq!(error.parameter(), Some("input.type"), "{item}");
        }
    }
}
