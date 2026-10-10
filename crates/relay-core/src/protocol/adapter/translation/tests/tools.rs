use super::*;
use serde_json::json;

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
fn generic_messages_translation_rejects_provider_owned_and_unknown_fields() {
    let response = json!({
        "id": "msg_invalid",
        "stop_reason": "end_turn",
        "content": [{"type": "thinking", "thinking": "plan", "signature": "opaque"}]
    });
    assert!(response::decode(WireApi::Messages, &response, "seed").is_err());

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
fn chat_bridge_flattens_codex_namespaces_and_restores_them_on_output() {
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
            {"type": "web_search"},
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
    assert_eq!(tools.len(), 2);
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
