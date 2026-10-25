use super::*;
use serde_json::json;

#[test]
fn responses_upstream_preserves_foreign_tool_strict_defaults_and_optional_fields() {
    let schema = json!({
        "type":"object",
        "properties":{"query":{"type":"string"},"limit":{"type":"integer"}},
        "required":["query"],
        "additionalProperties":false
    });
    for client in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        for strict in [None, Some(false), Some(true)] {
            if client == WireApi::Gemini && strict.is_some() {
                continue;
            }
            let mut parameters = schema.clone();
            if strict == Some(true) {
                parameters["required"] = json!(["query", "limit"]);
            }
            let mut request = input(client);
            let mut declaration = match client {
                WireApi::ChatCompletions => json!({"name":"lookup","parameters":parameters}),
                WireApi::Messages => json!({"name":"lookup","input_schema":parameters}),
                WireApi::Gemini => json!({"name":"lookup","parametersJsonSchema":parameters}),
                _ => unreachable!(),
            };
            if let Some(strict) = strict {
                declaration["strict"] = strict.into();
            }
            request["tools"] = match client {
                WireApi::GeminiInteractions => unreachable!("native-only fixture is separate"),
                WireApi::ChatCompletions => json!([{"type":"function","function":declaration}]),
                WireApi::Messages => json!([declaration]),
                WireApi::Gemini => json!([{"functionDeclarations":[declaration]}]),
                _ => unreachable!(),
            };
            for stream in [false, true] {
                let prepared = prepare(client, WireApi::Responses, &request, stream);
                let tool = &prepared.upstream_body()["tools"][0];
                assert_eq!(tool["strict"], strict.unwrap_or(false), "{client:?}");
                assert_eq!(tool["parameters"], parameters, "{client:?}");
            }
            assert_eq!(
                prepare(client, client, &request, false).upstream_body()["tools"],
                request["tools"]
            );
        }
    }
    let native =
        json!({"input":"test","tools":[{"type":"function","name":"lookup","parameters":schema}]});
    let prepared = prepare(WireApi::Responses, WireApi::Responses, &native, false);
    assert!(prepared.upstream_body()["tools"][0].get("strict").is_none());
}

#[test]
fn required_tool_choices_reject_text_only_success_without_rejecting_limits_or_refusals() {
    for client in WireApi::ALL {
        for named in [false, true] {
            let mut request = input(client);
            let schema = json!({"type":"object","properties":{}});
            request["tools"] = match client {
                WireApi::GeminiInteractions => unreachable!("native-only fixture is separate"),
                WireApi::Responses => {
                    json!([{"type":"function","name":"lookup","parameters":schema}])
                }
                WireApi::ChatCompletions => {
                    json!([{"type":"function","function":{"name":"lookup","parameters":schema}}])
                }
                WireApi::Messages => json!([{"name":"lookup","input_schema":schema}]),
                WireApi::Gemini => {
                    json!([{"functionDeclarations":[{"name":"lookup","parameters":schema}]}])
                }
            };
            match client {
                WireApi::GeminiInteractions => unreachable!("native-only fixture is separate"),
                WireApi::Responses => {
                    request["tool_choice"] = if named {
                        json!({"type":"function","name":"lookup"})
                    } else {
                        json!("required")
                    }
                }
                WireApi::ChatCompletions => {
                    request["tool_choice"] = if named {
                        json!({"type":"function","function":{"name":"lookup"}})
                    } else {
                        json!("required")
                    }
                }
                WireApi::Messages => {
                    request["tool_choice"] = if named {
                        json!({"type":"tool","name":"lookup"})
                    } else {
                        json!({"type":"any"})
                    }
                }
                WireApi::Gemini => {
                    request["toolConfig"] = json!({"functionCallingConfig":{"mode":"ANY"}});
                    if named {
                        request["toolConfig"]["functionCallingConfig"]["allowedFunctionNames"] =
                            json!(["lookup"]);
                    }
                }
            }
            for upstream in WireApi::ALL
                .into_iter()
                .filter(|upstream| *upstream != client)
            {
                let error = prepare(client, upstream, &request, false)
                    .translate_response_bytes(&serde_json::to_vec(&output(upstream)).unwrap())
                    .unwrap_err();
                assert_eq!(
                    error.parameter(),
                    Some("tool_choice"),
                    "{client:?} -> {upstream:?}"
                );
                let mut bridge = prepare(client, upstream, &request, true)
                    .into_stream_bridge()
                    .unwrap();
                for bytes in sse(upstream).chunks(5) {
                    bridge.push(bytes);
                }
                bridge.finish();
                assert!(bridge.completed().is_none());
                let wire = String::from_utf8(
                    std::iter::from_fn(|| bridge.pop_output())
                        .flatten()
                        .collect(),
                )
                .unwrap();
                assert!(
                    wire.contains("\"param\":\"tool_choice\""),
                    "{client:?} -> {upstream:?}: {wire}"
                );

                let mut bridge = prepare(client, upstream, &request, true)
                    .into_stream_bridge()
                    .unwrap();
                bridge.push(&tool_stream(upstream));
                bridge.finish();
                assert!(bridge.completed().is_some(), "{client:?} -> {upstream:?}");

                let mut limited = output(upstream);
                let mut refused = output(upstream);
                match upstream {
                    WireApi::GeminiInteractions => unreachable!("native-only fixture is separate"),
                    WireApi::Responses => {
                        limited["status"] = "incomplete".into();
                        limited["incomplete_details"] = json!({"reason":"max_output_tokens"});
                        refused["output"][0]["content"] =
                            json!([{"type":"refusal","refusal":"Synthetic refusal"}]);
                    }
                    WireApi::ChatCompletions => {
                        limited["choices"][0]["finish_reason"] = "length".into();
                        refused["choices"][0]["message"]["refusal"] = "Synthetic refusal".into();
                        refused["choices"][0]["message"]["content"] = Value::Null;
                    }
                    WireApi::Messages => {
                        limited["stop_reason"] = "max_tokens".into();
                        refused["stop_reason"] = "refusal".into();
                    }
                    WireApi::Gemini => {
                        limited["candidates"][0]["finishReason"] = "MAX_TOKENS".into();
                        refused["candidates"][0]["finishReason"] = "SAFETY".into();
                    }
                }
                for payload in [limited, refused] {
                    prepare(client, upstream, &request, false)
                        .translate_response_bytes(&serde_json::to_vec(&payload).unwrap())
                        .unwrap();
                }
            }
        }
    }
}

#[test]
fn nested_namespace_tools_roundtrip_across_all_responses_bridges() {
    let request = json!({"input":"Find the file", "max_output_tokens":64,"tools":[{
        "type":"namespace","name":"workspace","description":"Outer scope","tools":[{
            "type":"namespace","name":"files","description":"Inner scope","tools":[
                {"type":"function","name":"lookup","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}},
                {"type":"custom","name":"patch","format":{"type":"grammar","syntax":"regex","definition":"[a-z]+"}}
            ]
        }]
    }]});
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let prepared = prepare(WireApi::Responses, upstream, &request, false);
        let body = prepared.upstream_body();
        let name = match upstream {
            WireApi::ChatCompletions => &body["tools"][0]["function"]["name"],
            WireApi::Messages => &body["tools"][0]["name"],
            WireApi::Gemini => &body["tools"][0]["functionDeclarations"][0]["name"],
            _ => unreachable!(),
        }
        .as_str()
        .unwrap();
        assert!(body.to_string().contains("Outer scope Inner scope"));
        assert!(body.to_string().contains("[a-z]+"));
        let response = match upstream {
            WireApi::ChatCompletions => {
                json!({"id":"chat_nested","choices":[{"message":{"role":"assistant","tool_calls":[{"id":"call_nested","type":"function","function":{"name":name,"arguments":"{\"path\":\"file.txt\"}"}}]},"finish_reason":"tool_calls"}]})
            }
            WireApi::Messages => {
                json!({"id":"msg_nested","content":[{"type":"tool_use","id":"call_nested","name":name,"input":{"path":"file.txt"}}],"stop_reason":"tool_use"})
            }
            WireApi::Gemini => {
                json!({"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"id":"call_nested","name":name,"args":{"path":"file.txt"}}}]},"finishReason":"STOP"}]})
            }
            _ => unreachable!(),
        };
        let translated = prepared
            .translate_response_bytes(&serde_json::to_vec(&response).unwrap())
            .unwrap()
            .unwrap();
        let call = &translated.response_body()["output"][0];
        assert_eq!(call["name"], "lookup");
        assert_eq!(call["namespace"], "workspace.files");
        let next = json!({"previous_response_id":translated.response_id(),"input":[{"type":"function_call_output","call_id":call["call_id"],"output":"found"}],"tools":request["tools"],"max_output_tokens":64});
        let continuation = SourceAdapter::between(WireApi::Responses, upstream)
            .unwrap()
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: &next,
                model: "test",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: Some(translated.continuation().unwrap().1.clone()),
                response_scope: "source-test",
                response_id_seed: "next",
            })
            .unwrap();
        assert!(continuation.upstream_body().to_string().contains("found"));
        continuation
            .translate_response_bytes(&serde_json::to_vec(&output(upstream)).unwrap())
            .unwrap()
            .unwrap();
    }
}

#[test]
fn responses_bridges_reject_malformed_controls_and_unknown_tool_fields() {
    for upstream in [WireApi::Messages, WireApi::Gemini, WireApi::ChatCompletions] {
        for request in [
            json!({"input":"test","reasoning":"high"}),
            json!({"input":"test","reasoning":{"effort":true}}),
            json!({"input":"test","parallel_tool_calls":"false"}),
            json!({"input":"test","tools":[{"type":"function","name":"lookup","vendor_behavior":true}]}),
        ] {
            let result = SourceAdapter::between(WireApi::Responses, upstream)
                .unwrap()
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Responses,
                    request: &request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "test",
                    response_id_seed: "test",
                });
            assert!(result.is_err(), "{upstream:?}: {request}");
        }
    }
}
#[test]
fn generic_messages_translation_retains_signed_thinking_and_rejects_unknown_fields() {
    let response = json!({
        "id": "msg_invalid",
        "stop_reason": "end_turn",
        "content": [{"type": "thinking", "thinking": "plan", "signature": "opaque"}]
    });
    let decoded = response::decode(WireApi::Messages, &response, "seed").unwrap();
    assert_eq!(
        decoded.native_output,
        response["content"].as_array().unwrap().clone()
    );
    assert!(matches!(&decoded.blocks[0], Block::Reasoning(text) if text == "plan"));

    let mut bridge = prepare(
        WireApi::ChatCompletions,
        WireApi::Messages,
        &input(WireApi::ChatCompletions),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    for event in [
        json!({"type":"message_start","message":{"id":"msg_invalid","usage":{"input_tokens":1}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"","vendor_extension":true}}),
    ] {
        bridge.push(format!("data: {event}\n\n").as_bytes());
    }
    bridge.finish();
    assert!(bridge.completed().is_none());
}
#[test]
fn parallel_responses_calls_stay_in_one_assistant_turn_for_chat_upstream() {
    let request = json!({"model":"test","input":[
        {"role":"user","content":"Look up both values"},
        {"type":"function_call","call_id":"call_first","name":"lookup","arguments":"{\"value\":1}"},
        {"type":"function_call","call_id":"call_second","name":"lookup","arguments":"{\"value\":2}"},
        {"type":"function_call_output","call_id":"call_first","output":"one"},
        {"type":"function_call_output","call_id":"call_second","output":"two"}
    ]});
    let prepared = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &request,
        false,
    );
    let messages = prepared.upstream_body()["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[1]["role"], "assistant");
    assert_eq!(messages[1]["tool_calls"][0]["id"], "call_first");
    assert_eq!(messages[1]["tool_calls"][1]["id"], "call_second");
    assert_eq!(messages[2]["tool_call_id"], "call_first");
    assert_eq!(messages[3]["tool_call_id"], "call_second");
}

#[test]
fn parallel_history_keeps_every_call_and_result_across_all_upstreams() {
    let mut input = vec![json!({"role":"user","content":"Read three independent values"})];
    for index in 0..3 {
        input.push(
            json!({"type":"function_call","call_id":format!("call_batch_{index}"),
            "name":"lookup","arguments":json!({"value":index}).to_string()}),
        );
    }
    // Results need not arrive in dispatch order. Pair by identity, not index.
    for index in [2, 0, 1] {
        input.push(
            json!({"type":"function_call_output","call_id":format!("call_batch_{index}"),
            "output":format!("result-{index}")}),
        );
    }
    let request = json!({"model":"synthetic","input":input,"parallel_tool_calls":true});
    let capabilities = crate::model_metadata::ModelCapabilities {
        reasoning: Some(false),
        ..Default::default()
    };
    for upstream in WireApi::ALL {
        let prepared = SourceAdapter::between(WireApi::Responses, upstream)
            .unwrap()
            .prepare_request_with_capabilities(
                AdapterRequestContext {
                    client_wire_api: WireApi::Responses,
                    request: &request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "source-test",
                    response_id_seed: "request-test",
                },
                &capabilities,
            )
            .unwrap();
        let body = prepared.upstream_body();
        let calls: Vec<&Value> = match upstream {
            WireApi::GeminiInteractions => unreachable!("native-only fixture is separate"),
            WireApi::Responses => body["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["type"] == "function_call")
                .collect(),
            WireApi::ChatCompletions => body["messages"][1]["tool_calls"]
                .as_array()
                .unwrap()
                .iter()
                .collect(),
            WireApi::Messages => body["messages"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|message| message["content"].as_array().into_iter().flatten())
                .filter(|block| block["type"] == "tool_use")
                .collect(),
            WireApi::Gemini => body["contents"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|message| message["parts"].as_array().into_iter().flatten())
                .filter_map(|part| part.get("functionCall"))
                .collect(),
        };
        assert_eq!(calls.len(), 3, "{upstream:?}");
        for (index, call) in calls.iter().enumerate() {
            let id = match upstream {
                WireApi::Responses => &call["call_id"],
                _ => &call["id"],
            };
            assert_eq!(id, &json!(format!("call_batch_{index}")), "{upstream:?}");
        }
        let serialized = body.to_string();
        for index in 0..3 {
            assert_eq!(
                serialized.matches(&format!("result-{index}")).count(),
                1,
                "{upstream:?}"
            );
        }
    }
}
#[test]
fn gemini_calls_without_ids_are_unique_across_turns_and_keep_pairing() {
    let mut request = tool_history(WireApi::Gemini);
    for item in request["contents"].as_array_mut().unwrap() {
        for part in item["parts"].as_array_mut().unwrap() {
            for field in ["functionCall", "functionResponse"] {
                if let Some(value) = part.get_mut(field).and_then(Value::as_object_mut) {
                    value.remove("id");
                }
            }
        }
    }
    let body = prepare(WireApi::Gemini, WireApi::ChatCompletions, &request, false);
    let messages = body.upstream_body()["messages"].as_array().unwrap();
    let calls = messages
        .iter()
        .filter_map(|item| item.pointer("/tool_calls/0/id"))
        .collect::<Vec<_>>();
    let results = messages
        .iter()
        .filter_map(|item| item.get("tool_call_id"))
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 2);
    assert_ne!(calls[0], calls[1]);
    assert_eq!(calls, results);
}
#[test]
fn orphan_tool_results_are_rejected_for_every_conversion() {
    for client in WireApi::ALL {
        let mut request = tool_history(client);
        let key = match client {
            WireApi::Responses => "input",
            WireApi::Gemini => "contents",
            _ => "messages",
        };
        let history = request[key].as_array_mut().unwrap();
        let last = history.pop().unwrap();
        *history = vec![last];
        for upstream in WireApi::ALL {
            if upstream == client {
                continue;
            }
            let result = SourceAdapter::between(client, upstream)
                .unwrap()
                .prepare_request(AdapterRequestContext {
                    client_wire_api: client,
                    request: &request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "source",
                    response_id_seed: "next",
                });
            assert!(result.is_err(), "{client:?} -> {upstream:?}");
        }
    }
}
#[test]
fn chat_bridge_keeps_apply_patch_as_a_custom_tool_call() {
    let request = json!({
        "model": "grok",
        "input": [
            {"type": "message", "role": "user", "content": "Edit the file"},
            {"type": "custom_tool_call", "call_id": "call_patch", "name": "apply_patch", "input": "*** Begin Patch\n*** End Patch"},
            {"type": "custom_tool_call_output", "call_id": "call_patch", "output": "applied"}
        ],
        "tools": [{
            "type": "custom",
            "name": "apply_patch",
            "description": "Apply a patch",
            "format": {"type": "text"}
        }]
    });
    let prepared = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &request,
        false,
    );
    let body = prepared.upstream_body().to_string();
    assert!(body.contains("apply_patch"));
    assert!(body.contains("Begin Patch"));
    assert!(body.contains("\"input\""));
    let upstream = json!({
        "id": "chat_test",
        "object": "chat.completion",
        "model": "test",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_new",
                    "type": "function",
                    "function": {
                        "name": "apply_patch",
                        "arguments": "{\"input\":\"*** Begin Patch\\n+line\\n*** End Patch\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let translated = prepared
        .translate_response_bytes(&serde_json::to_vec(&upstream).unwrap())
        .unwrap()
        .unwrap();
    let item = &translated.response_body()["output"][0];
    assert_eq!(item["type"], "custom_tool_call");
    assert_eq!(item["name"], "apply_patch");
    assert_eq!(item["call_id"], "call_new");
    assert!(item["id"].as_str().unwrap().starts_with("ctc_"));
    assert!(item["input"].as_str().unwrap().contains("Begin Patch"));
    assert!(!translated
        .response_body()
        .to_string()
        .contains("function_call"));
}
#[test]
fn chat_bridge_flattens_selected_codex_namespace_and_restores_it_on_output() {
    let request = json!({
        "model": "grok",
        "input": [
            {"type": "additional_tools", "tools": [{
                "type": "namespace",
                "name": "mcp__synthetic",
                "description": "Synthetic server",
                "tools": [{
                    "type": "function",
                    "name": "lookup",
                    "description": "Look something up",
                    "parameters": {"type": "object"},
                    "defer_loading": true
                }]
            }]},
            {"type": "message", "role": "user", "content": "go"}
        ],
        "tools": [
            {
                "type": "function",
                "name": "shell",
                "parameters": {"type": "object"},
                "allowed_callers": ["direct"]
            }
        ],
        "tool_choice": {"type": "function", "namespace": "mcp__synthetic", "name": "lookup"}
    });
    let prepared = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &request,
        false,
    );
    let tools = prepared.upstream_body()["tools"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(tools.len(), 1);
    let flattened = tools
        .iter()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .find(|name| name.starts_with("relay_ns_"))
        .unwrap()
        .to_owned();
    assert_eq!(
        prepared.upstream_body()["tool_choice"]["function"]["name"],
        flattened.as_str()
    );
    let upstream = json!({
        "id": "chat_test",
        "object": "chat.completion",
        "model": "test",
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": "call_ns",
                    "type": "function",
                    "function": {"name": flattened, "arguments": "{}"}
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let translated = prepared
        .translate_response_bytes(&serde_json::to_vec(&upstream).unwrap())
        .unwrap()
        .unwrap();
    let item = &translated.response_body()["output"][0];
    assert_eq!(item["type"], "function_call");
    assert_eq!(item["name"], "lookup");
    assert_eq!(item["namespace"], "mcp__synthetic");
}

fn custom_response_events() -> Vec<Value> {
    let item = json!({"type":"custom_tool_call","id":"ctc_synthetic","call_id":"call_patch","name":"apply_patch","input":""});
    let complete = json!({"type":"custom_tool_call","id":"ctc_synthetic","call_id":"call_patch","name":"apply_patch","input":"line one\nline two"});
    vec![
        json!({"type":"response.output_item.added","output_index":0,"item":item}),
        json!({"type":"response.custom_tool_call_input.delta","output_index":0,"item_id":"ctc_synthetic","delta":"line one\n"}),
        json!({"type":"response.custom_tool_call_input.delta","output_index":0,"item_id":"ctc_synthetic","delta":"line two"}),
        json!({"type":"response.custom_tool_call_input.done","output_index":0,"item_id":"ctc_synthetic","input":"line one\nline two"}),
        json!({"type":"response.output_item.done","output_index":0,"item":complete}),
        json!({"type":"response.completed","response":{"status":"completed","output":[complete]}}),
    ]
}

fn custom_chat_request() -> Value {
    json!({"messages":[{"role":"user","content":"Apply the patch"}],
        "tools":[{"type":"function","function":{"name":"apply_patch","parameters":{"type":"object","properties":{"input":{"type":"string"}}}}}]})
}

#[test]
fn responses_custom_input_is_wrapped_once_in_json_and_fragmented_streams() {
    let events = custom_response_events();
    for stream in [false, true] {
        let prepared = prepare(
            WireApi::ChatCompletions,
            WireApi::Responses,
            &custom_chat_request(),
            stream,
        );
        let body = if stream {
            let mut bridge = prepared.into_stream_bridge().unwrap();
            for event in &events {
                for fragment in format!("data: {event}\n\n").as_bytes().chunks(3) {
                    bridge.push(fragment);
                }
            }
            bridge.finish();
            bridge
                .completed()
                .expect("custom input stream completes")
                .response_body
                .clone()
        } else {
            prepared
                .translate_response_bytes(
                    &serde_json::to_vec(&events.last().unwrap()["response"]).unwrap(),
                )
                .unwrap()
                .unwrap()
                .response_body()
                .clone()
        };
        let call = &body["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(call["id"], "call_patch");
        assert_eq!(call["function"]["name"], "apply_patch");
        let arguments: Value =
            serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
        assert_eq!(arguments, json!({"input":"line one\nline two"}));
        assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
    }
}

#[test]
fn responses_custom_input_rejects_conflicting_identity_done_and_terminal_data() {
    for mutation in 0..5 {
        let mut events = custom_response_events();
        match mutation {
            0 => events[2]["item_id"] = "different_item".into(),
            1 => events[3]["input"] = "different input".into(),
            2 => events[4]["item"]["call_id"] = "different_call".into(),
            3 => events[5]["response"]["output"][0]["input"] = "different input".into(),
            4 => events.insert(4, events[2].clone()),
            _ => unreachable!(),
        }
        let mut bridge = prepare(
            WireApi::ChatCompletions,
            WireApi::Responses,
            &custom_chat_request(),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        for event in events {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.finish();
        assert!(bridge.completed().is_none(), "mutation {mutation}");
        let output = std::iter::from_fn(|| bridge.pop_output())
            .flatten()
            .collect::<Vec<_>>();
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("adapter_upstream_stream_invalid"));
    }
}
