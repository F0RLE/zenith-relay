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
    try_prepare_discovery(upstream, request, stream)
        .unwrap_or_else(|error| panic!("Messages -> {upstream:?}: {error}"))
}

pub(super) fn try_prepare_discovery(
    upstream: WireApi,
    request: &Value,
    stream: bool,
) -> Result<crate::PreparedAdapterRequest, crate::AdapterError> {
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
}

#[test]
fn messages_tool_search_keeps_responses_prefix_stable_after_discovery() {
    for stream in [false, true] {
        let initial = prepare_discovery(WireApi::Responses, &catalog(), stream);
        let request = discovered();
        let original = request.clone();
        let loaded = prepare_discovery(WireApi::Responses, &request, stream);
        let initial = initial.upstream_body();
        let loaded = loaded.upstream_body();
        assert_eq!(request, original);
        assert_eq!(loaded["tools"], initial["tools"]);
        let prefix = initial["input"].as_array().unwrap();
        let input = loaded["input"].as_array().unwrap();
        assert_eq!(&input[..prefix.len()], prefix);
        assert_eq!(input[prefix.len()]["call_id"], "search_1");
        assert_eq!(input[prefix.len() + 1]["call_id"], "search_1");
        assert_eq!(
            input[prefix.len() + 2],
            json!({"type":"additional_tools","role":"developer","tools":[{
                "type":"function","name":"lookup","description":"Lookup a synthetic record","strict":false,
                "parameters":request["tools"][1]["input_schema"]
            }]})
        );
    }
}

#[test]
fn messages_tool_search_siwc_keeps_unused_initial_tools_in_the_cache_prefix() {
    let mut bodies = Vec::new();
    for mut request in [catalog(), discovered()] {
        // This fixture isolates tool loading; SIWC rejects explicit output caps.
        request.as_object_mut().unwrap().remove("max_tokens");
        request["tools"].as_array_mut().unwrap().push(json!({
            "name":"always_available", "input_schema":{"type":"object","properties":{}}
        }));
        let prepared = prepare_discovery(WireApi::Responses, &request, true);
        let mut body = prepared.upstream_body().clone();
        let input = body["input"].clone();
        crate::providers::chatgpt::prepare_siwc_request(&mut body).unwrap();
        assert_eq!(
            body["input"], input,
            "SIWC must not relocate initial declarations"
        );
        let initial = body["input"][0]["tools"].as_array().unwrap();
        assert_eq!(initial.len(), 2);
        assert_eq!(initial[0]["name"], "ToolSearch");
        assert_eq!(initial[1]["name"], "always_available");
        bodies.push(body);
    }
    assert_eq!(bodies[0]["tools"], bodies[1]["tools"]);
    let prefix = bodies[0]["input"].as_array().unwrap();
    assert_eq!(
        &bodies[1]["input"].as_array().unwrap()[..prefix.len()],
        prefix
    );
    assert_eq!(
        bodies[1]["input"][prefix.len() + 2]["tools"][0]["name"],
        "lookup"
    );
}

#[test]
fn messages_tool_search_emits_each_schema_once_without_rewriting_earlier_turns() {
    let first = prepare_discovery(WireApi::Responses, &discovered(), false);
    let mut request = discovered();
    request["messages"].as_array_mut().unwrap().extend([
        json!({"role":"assistant","content":[{"type":"tool_use","id":"search_2","name":"ToolSearch","input":{"query":"more"}}]}),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"search_2","content":[
            {"type":"tool_reference","tool_name":"lookup"},
            {"type":"tool_reference","tool_name":"unused"},
            {"type":"tool_reference","tool_name":"unused"}
        ]}]}),
        json!({"role":"assistant","content":[{"type":"tool_use","id":"lookup_1","name":"lookup","input":{"id":7}}]}),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"lookup_1","content":"synthetic result"}]}),
    ]);
    let next = prepare_discovery(WireApi::Responses, &request, false);
    let first = first.upstream_body();
    let next = next.upstream_body();
    assert_eq!(next["tools"], first["tools"]);
    let first_input = first["input"].as_array().unwrap();
    assert_eq!(
        &next["input"].as_array().unwrap()[..first_input.len()],
        first_input
    );
    let additions: Vec<_> = next["input"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|item| item["type"] == "additional_tools")
        .collect();
    assert_eq!(additions.len(), 3);
    assert_eq!(additions[0]["tools"][0]["name"], "ToolSearch");
    assert_eq!(additions[1]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(additions[1]["tools"][0]["name"], "lookup");
    assert_eq!(additions[2]["tools"].as_array().unwrap().len(), 1);
    assert_eq!(additions[2]["tools"][0]["name"], "unused");
    assert_eq!(next["input"][first_input.len() + 1]["call_id"], "search_2");
    assert_eq!(next["input"][first_input.len() + 2], *additions[2]);
}

#[test]
fn messages_tool_search_compacted_and_forced_tools_keep_their_original_schema() {
    let mut request = catalog();
    request["system"] = json!("Synthetic instructions");
    request["tool_choice"] = json!({"type":"tool","name":"lookup"});
    let forced = prepare_discovery(WireApi::Responses, &request, false);
    let body = forced.upstream_body();
    assert!(body["tools"].as_array().unwrap().is_empty());
    assert_eq!(body["input"][0]["tools"][0]["name"], "ToolSearch");
    assert_eq!(body["input"][1]["role"], "system");
    assert_eq!(body["input"][3]["type"], "additional_tools");
    assert_eq!(
        body["input"][3]["tools"][0]["parameters"],
        request["tools"][1]["input_schema"]
    );
    assert_eq!(
        body["tool_choice"],
        json!({"type":"function","name":"lookup"})
    );
    request["messages"].as_array_mut().unwrap().extend([
        json!({"role":"assistant","content":[{"type":"tool_use","id":"lookup_1","name":"lookup","input":{"id":7}}]}),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"lookup_1","content":"synthetic result"}]}),
    ]);
    let compacted = prepare_discovery(WireApi::Responses, &request, false);
    let body = compacted.upstream_body();
    assert_eq!(body["input"][3]["type"], "additional_tools");
    assert_eq!(body["input"][4]["call_id"], "lookup_1");
    assert_eq!(
        body["input"][3]["tools"][0]["parameters"],
        request["tools"][1]["input_schema"]
    );
}

#[test]
fn messages_tool_search_respects_later_inline_revocation_and_schema_replacement() {
    for replacement in [
        None,
        Some(json!({"name":"lookup", "input_schema":{
            "type":"object","properties":{"value":{"type":"boolean"}},"required":["value"]
        }})),
    ] {
        let mut request = discovered();
        let update = replacement.as_ref().map_or_else(
            || json!({"type":"tool_removal","tool":{"type":"tool_reference","name":"lookup"}}),
            |definition| json!({"type":"tool_addition","tool":{"type":"tool_definition","definition":definition}}),
        );
        request["messages"].as_array_mut().unwrap().extend([
            json!({"role":"system","content":[update]}),
            json!({"role":"assistant","content":"Synthetic acknowledgement"}),
            json!({"role":"user","content":"Continue"}),
        ]);
        let prepared = prepare_discovery(WireApi::Responses, &request, false);
        let body = prepared.upstream_body();
        assert!(
            !body["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["type"] == "additional_tools")
        );
        let lookup = body["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "lookup");
        if let Some(definition) = replacement {
            assert_eq!(lookup.unwrap()["parameters"], definition["input_schema"]);
        } else {
            assert!(
                lookup.is_none(),
                "discovery history must not restore a withdrawn tool"
            );
        }
    }
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
    assert!(
        decode::request(WireApi::Messages, &request)
            .unwrap()
            .tools
            .iter()
            .any(|tool| tool.name == "lookup")
    );
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

#[test]
fn inline_tools_json_and_sse_complete_call_result_and_followup_cycle() {
    for upstream in [WireApi::Responses, WireApi::ChatCompletions] {
        for stream in [false, true] {
            let mut request = super::messages_inline_tools::fixture();
            let arguments = json!({"value":"synthetic"});
            let reply = tool_reply(upstream, "new_call", "new", &arguments);
            let translated = client_reply(
                upstream,
                &request,
                stream,
                &reply,
                reply_stream(upstream, &reply),
            );
            assert_eq!(translated["content"][0]["id"], "new_call");
            assert_eq!(translated["content"][0]["name"], "new");
            assert_eq!(translated["content"][0]["input"], arguments);
            request["messages"].as_array_mut().unwrap().extend([
                json!({"role":"assistant","content":translated["content"]}),
                json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"new_call","content":"synthetic result"}]}),
            ]);
            let decoded = decode::request(WireApi::Messages, &request).unwrap();
            assert_eq!(
                decoded
                    .tools
                    .iter()
                    .map(|tool| tool.name.as_str())
                    .collect::<Vec<_>>(),
                ["later", "new"]
            );
            let reply = client_reply(upstream, &request, stream, &output(upstream), sse(upstream));
            assert_eq!(reply["stop_reason"], "end_turn");
            assert_eq!(reply["content"][0]["text"], "Hi");
        }
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
                0
            );
            let additions: Vec<_> = prepared.upstream_body()["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|item| item["type"] == "additional_tools")
                .collect();
            assert_eq!(additions.len(), 2);
            assert_eq!(additions[0]["tools"][0]["name"], "ToolSearch");
            assert_eq!(additions[1]["tools"][0]["name"], "lookup");
            assert_eq!(
                additions[1]["tools"][0]["parameters"],
                request["tools"][1]["input_schema"]
            );
            assert_eq!(
                prepared.upstream_body()["input"][2]["encrypted_content"],
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
        assert!(
            !completed
                .response_body
                .to_string()
                .contains("synthetic-private")
        );
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
