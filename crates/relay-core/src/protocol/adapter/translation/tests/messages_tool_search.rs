use super::*;

fn catalog() -> Value {
    json!({
        "model":"test", "max_tokens":64,
        "tools":[
            {"name":"ToolSearch", "input_schema":{"type":"object","properties":{"query":{"type":"string"}}}},
            {"name":"lookup", "description":"Lookup a synthetic record", "defer_loading":true,
             "input_schema":{"type":"object","properties":{"id":{"type":"integer"}},"required":["id"],"additionalProperties":false}},
            {"name":"unused", "defer_loading":true, "input_schema":{"type":"object","properties":{}}}
        ],
        "messages":[{"role":"user","content":"Find the record"}]
    })
}

fn discovered() -> Value {
    let mut request = catalog();
    request["messages"].as_array_mut().unwrap().extend([
        json!({"role":"assistant","content":[{"type":"tool_use","id":"search_1","name":"ToolSearch","input":{"query":"lookup"}}]}),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"search_1","content":[
            {"type":"text","text":"Found one tool"},
            {"type":"tool_reference","tool_name":"lookup"}
        ]}]}),
    ]);
    request
}

fn prepare_discovery(
    upstream: WireApi,
    request: &Value,
    stream: bool,
) -> crate::PreparedAdapterRequest {
    // Isolate portable discovery from opaque-reasoning ownership: the fixture
    // has no provider reasoning state and does not claim one exists.
    SourceAdapter::between(WireApi::Messages, upstream)
        .unwrap()
        .prepare_request_with_capabilities(
            AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request,
                model: "test",
                stream,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "tool-search-test",
                response_id_seed: "tool-search-test",
            },
            &crate::model_metadata::ModelCapabilities {
                reasoning: Some(false),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("Messages -> {upstream:?}: {error}"))
}

#[test]
fn messages_tool_search_loads_only_explicit_client_discoveries() {
    let request = discovered();
    let original = request.clone();
    let decoded = decode::request(WireApi::Messages, &request).unwrap();
    assert_eq!(request, original);
    assert_eq!(
        decoded
            .tools
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>(),
        ["ToolSearch", "lookup"]
    );
    assert_eq!(
        decoded.tools[1].parameters,
        request["tools"][1]["input_schema"]
    );
    let Block::ToolResult { id, content, .. } = &decoded.messages[2].blocks[0] else {
        panic!("discovery must remain a tool result");
    };
    assert_eq!(id, "search_1");
    assert!(
        matches!(&content[1], Block::Text(text) if serde_json::from_str::<Value>(text).unwrap() == json!({"type":"tool_reference","tool_name":"lookup"}))
    );
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        for stream in [false, true] {
            let prepared = prepare_discovery(upstream, &request, stream);
            let body = prepared.upstream_body().to_string();
            assert!(!body.contains("defer_loading"), "{upstream:?}");
            assert!(!body.contains("\"unused\""), "{upstream:?}");
            assert!(body.contains("search_1"), "{upstream:?}");
        }
    }
}

#[test]
fn messages_tool_search_does_not_eagerly_load_the_deferred_catalog() {
    let request = catalog();
    let decoded = decode::request(WireApi::Messages, &request).unwrap();
    assert_eq!(decoded.tools.len(), 1);
    assert_eq!(decoded.tools[0].name, "ToolSearch");
    let mut forced = request;
    forced["tool_choice"] = json!({"type":"tool","name":"lookup"});
    assert_eq!(
        decode::request(WireApi::Messages, &forced)
            .unwrap()
            .tools
            .len(),
        2
    );
}

#[test]
fn messages_tool_search_keeps_loaded_tools_for_the_next_real_call() {
    let mut request = discovered();
    request["messages"].as_array_mut().unwrap().extend([
        json!({"role":"assistant","content":[{"type":"tool_use","id":"lookup_1","name":"lookup","input":{"id":7}}]}),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"lookup_1","content":"synthetic result"}]}),
    ]);
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let _ = prepare_discovery(upstream, &request, false);
    }
    // A compacted discovery may be absent while the real tool call remains.
    request["messages"].as_array_mut().unwrap().drain(1..3);
    assert!(decode::request(WireApi::Messages, &request)
        .unwrap()
        .tools
        .iter()
        .any(|tool| tool.name == "lookup"));
}

#[test]
fn messages_tool_search_rejects_unknown_references_and_invalid_catalogs() {
    let mut invalid = Vec::new();
    let mut request = discovered();
    request["messages"][2]["content"][0]["content"][1]["tool_name"] = json!("missing");
    invalid.push(request);
    let mut request = discovered();
    request["messages"][2]["content"][0]["is_error"] = json!(true);
    invalid.push(request);
    let mut request = discovered();
    request["messages"][2]["role"] = json!("assistant");
    invalid.push(request);
    let mut request = catalog();
    request["tools"][1]["defer_loading"] = json!("true");
    invalid.push(request);
    let mut request = catalog();
    request["tools"][2]["name"] = json!("lookup");
    invalid.push(request);
    let mut request = catalog();
    request["tools"][2]["unknown_extension"] = json!(true);
    invalid.push(request);
    let mut request = catalog();
    request["tools"][0]["type"] = json!("tool_search_tool_regex_20251119");
    invalid.push(request);
    let mut request = catalog();
    request["tools"][0]["defer_loading"] = json!(true);
    invalid.push(request);
    let mut request = catalog();
    request["tools"][1]["cache_control"] = json!({"type":"ephemeral"});
    invalid.push(request);
    let mut request = catalog();
    request["tools"][2]["input_schema"] = json!("not a schema");
    invalid.push(request);
    let mut request = discovered();
    request["messages"][2]["content"][0]["content"][1]["unknown"] = json!(true);
    invalid.push(request);
    let mut request = discovered();
    request["messages"][2]["content"][0]["content"][1]["tool_name"] = json!("");
    invalid.push(request);
    for request in invalid {
        assert!(decode::request(WireApi::Messages, &request).is_err());
    }
}

#[test]
fn messages_tool_search_native_route_preserves_discovery_without_filtering() {
    let request = discovered();
    let native = prepare(WireApi::Messages, WireApi::Messages, &request, false);
    assert!(native.is_passthrough());
    assert_eq!(native.upstream_body()["tools"], request["tools"]);
    assert_eq!(native.upstream_body()["messages"], request["messages"]);
}

#[test]
fn messages_tool_search_does_not_repair_an_orphan_result() {
    let mut request = discovered();
    request["messages"][2]["content"][0]["tool_use_id"] = json!("not_called");
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let error = SourceAdapter::between(WireApi::Messages, upstream)
            .unwrap()
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &request,
                model: "test",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "tool-search-test",
                response_id_seed: "tool-search-test",
            });
        assert!(error.is_err(), "{upstream:?}");
    }
}

fn tool_reply(protocol: WireApi, id: &str, name: &str, arguments: &Value) -> Value {
    let mut reply = output(protocol);
    match protocol {
        WireApi::GeminiInteractions => unreachable!("native-only fixture is separate"),
        WireApi::Responses => {
            reply["output"] = json!([{"type":"function_call","id":"item_1","call_id":id,"name":name,"arguments":arguments.to_string()}]);
        }
        WireApi::ChatCompletions => {
            reply["choices"] = json!([{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"type":"function","id":id,"function":{"name":name,"arguments":arguments.to_string()}}]},"finish_reason":"tool_calls"}]);
        }
        WireApi::Gemini => {
            reply["candidates"] = json!([{"index":0,"content":{"role":"model","parts":[{"functionCall":{"id":id,"name":name,"args":arguments}}]},"finishReason":"STOP"}]);
        }
        WireApi::Messages => unreachable!(),
    }
    reply
}

fn reply_stream(protocol: WireApi, reply: &Value) -> Vec<u8> {
    let events = match protocol {
        WireApi::GeminiInteractions => unreachable!("native-only fixture is separate"),
        WireApi::Responses => {
            let call = &reply["output"][0];
            let mut start = call.clone();
            start["arguments"] = json!("");
            vec![
                json!({"type":"response.created","response":{"id":"response_test","output":[]}}),
                json!({"type":"response.output_item.added","output_index":0,"item":start}),
                json!({"type":"response.function_call_arguments.delta","output_index":0,"delta":call["arguments"]}),
                json!({"type":"response.completed","response":reply}),
            ]
        }
        WireApi::ChatCompletions => vec![
            json!({"id":"chat_test","choices":[{"index":0,"delta":{"role":"assistant","tool_calls":[{"index":0,"id":reply["choices"][0]["message"]["tool_calls"][0]["id"],"type":"function","function":reply["choices"][0]["message"]["tool_calls"][0]["function"]}]},"finish_reason":null}]}),
            json!({"id":"chat_test","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":reply["usage"]}),
        ],
        WireApi::Gemini => vec![reply.clone()],
        WireApi::Messages => unreachable!(),
    };
    let mut wire = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    if protocol == WireApi::ChatCompletions {
        wire.push_str("data: [DONE]\n\n");
    }
    wire.into_bytes()
}

fn client_reply(
    upstream: WireApi,
    request: &Value,
    stream: bool,
    reply: &Value,
    wire: Vec<u8>,
) -> Value {
    let prepared = prepare_discovery(upstream, request, stream);
    assert!(!prepared.upstream_body().to_string().contains("\"unused\""));
    if !stream {
        return prepared
            .translate_response_bytes(&serde_json::to_vec(reply).unwrap())
            .unwrap()
            .unwrap()
            .response_body()
            .clone();
    }
    let mut bridge = prepared.into_stream_bridge().unwrap();
    for chunk in wire.chunks(3) {
        bridge.push(chunk);
    }
    bridge.finish();
    assert!(bridge.is_terminal(), "{upstream:?}");
    let completed = bridge.completed().unwrap_or_else(|| panic!("{upstream:?}"));
    let response = completed.response_body.clone();
    let mut emitted = Vec::new();
    while let Some(bytes) = bridge.pop_output() {
        emitted.extend(bytes);
    }
    let emitted = String::from_utf8(emitted).unwrap();
    assert!(!emitted.contains("adapter_upstream_stream_invalid"));
    assert_eq!(emitted.matches("event: message_stop").count(), 1);
    response
}

#[test]
fn messages_tool_search_json_and_sse_complete_discovery_call_result_cycle() {
    for upstream in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        for stream in [false, true] {
            let mut request = catalog();
            for (id, name, arguments, result) in [
                (
                    "search_1",
                    "ToolSearch",
                    json!({"query":"lookup"}),
                    json!([{"type":"tool_reference","tool_name":"lookup"}]),
                ),
                (
                    "lookup_1",
                    "lookup",
                    json!({"id":7}),
                    json!("synthetic result"),
                ),
            ] {
                let reply = tool_reply(upstream, id, name, &arguments);
                let translated = client_reply(
                    upstream,
                    &request,
                    stream,
                    &reply,
                    reply_stream(upstream, &reply),
                );
                assert_eq!(translated["stop_reason"], "tool_use");
                assert_eq!(translated["content"][0]["type"], "tool_use");
                assert_eq!(translated["content"][0]["id"], id);
                assert_eq!(translated["content"][0]["name"], name);
                assert_eq!(translated["content"][0]["input"], arguments);
                request["messages"].as_array_mut().unwrap().extend([
                    json!({"role":"assistant","content":translated["content"]}),
                    json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":result}]}),
                ]);
                let decoded = decode::request(WireApi::Messages, &request).unwrap();
                assert_eq!(
                    decoded
                        .tools
                        .iter()
                        .map(|tool| tool.name.as_str())
                        .collect::<Vec<_>>(),
                    ["ToolSearch", "lookup"]
                );
            }
            let reply = client_reply(upstream, &request, stream, &output(upstream), sse(upstream));
            assert_eq!(reply["stop_reason"], "end_turn");
            assert_eq!(reply["content"][0]["text"], "Hi");
        }
    }
}

#[test]
fn messages_tool_search_preserves_opaque_reasoning_ownership_across_discovery() {
    let mut request = catalog();
    request["thinking"] = json!({"type":"adaptive"});
    request["output_config"] = json!({"effort":"high"});
    let mut previous = None;
    for (id, name, args, result) in [
        (
            "search_1",
            "ToolSearch",
            json!({"query":"lookup"}),
            json!([{"type":"tool_reference","tool_name":"lookup"}]),
        ),
        (
            "lookup_1",
            "lookup",
            json!({"id":7}),
            json!("synthetic result"),
        ),
    ] {
        let prepared = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &request,
                model: "test",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: previous.take(),
                response_scope: "source-test",
                response_id_seed: id,
            })
            .unwrap();
        if id == "lookup_1" {
            assert_eq!(
                prepared.upstream_body()["tools"].as_array().unwrap().len(),
                2
            );
            assert_eq!(
                prepared.upstream_body()["input"][1]["encrypted_content"],
                "synthetic-private-search_1"
            );
        }
        let mut native = tool_reply(WireApi::Responses, id, name, &args);
        native["id"] = json!(format!("resp_{id}"));
        native["output"].as_array_mut().unwrap().insert(
            0,
            json!({"type":"reasoning","id":format!("rs_{id}"),"summary":[{"type":"summary_text","text":"Plan the synthetic call."}],"encrypted_content":format!("synthetic-private-{id}")}),
        );
        let response = prepared
            .translate_response_bytes(&serde_json::to_vec(&native).unwrap())
            .unwrap()
            .unwrap();
        let completed = response.translated_response();
        assert!(!completed
            .response_body
            .to_string()
            .contains("synthetic-private"));
        previous = Some(completed.continuation.clone());
        request["messages"].as_array_mut().unwrap().extend([
            json!({"role":"assistant","content":completed.response_body["content"]}),
            json!({"role":"user","content":[{"type":"tool_result","tool_use_id":id,"content":result}]}),
        ]);
        assert_eq!(
            continuation::request_key(WireApi::Messages, &request, "test").unwrap(),
            completed.continuation.history_key
        );
    }
    let completed = SourceAdapter::MessagesToResponses
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Messages,
            request: &request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous,
            response_scope: "source-test",
            response_id_seed: "final",
        })
        .unwrap()
        .translate_response_bytes(&serde_json::to_vec(&output(WireApi::Responses)).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(completed.response_body()["content"][0]["text"], "Hi");
}
