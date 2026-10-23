use super::*;

fn tool_request(client: WireApi) -> Value {
    let mut request = tool_history(client);
    let field = if client == WireApi::Gemini {
        "contents"
    } else {
        "messages"
    };
    let initial_messages = if client == WireApi::ChatCompletions {
        2
    } else {
        1
    };
    request[field]
        .as_array_mut()
        .unwrap()
        .truncate(initial_messages);
    request
}

fn tool_call() -> Value {
    json!({"type":"function_call","id":"fc_synthetic_item","call_id":"call_synthetic_execution",
        "name":"lookup","arguments":"{\"value\":2}","status":"completed"})
}

fn tool_response(call: &Value) -> Value {
    json!({"id":"resp_synthetic_tools","status":"completed","output":[call],
        "usage":{"input_tokens":11,"output_tokens":7}})
}

fn tool_events(call: &Value) -> Vec<Value> {
    let mut added = call.clone();
    added["arguments"] = "".into();
    added["status"] = "in_progress".into();
    vec![
        json!({"type":"response.created","response":{"id":"resp_synthetic_tools","output":[]}}),
        json!({"type":"response.output_item.added","output_index":0,"item":added}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":call["id"],"delta":"{\"value\":"}),
        json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":call["id"],"delta":"2}"}),
        json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":call["id"],"arguments":call["arguments"]}),
        json!({"type":"response.output_item.done","output_index":0,"item":call}),
        json!({"type":"response.completed","response":tool_response(call)}),
    ]
}

#[test]
fn responses_tool_execution_controls_are_not_silently_dropped() {
    for (extension, parameter) in [
        (json!({"async":true}), "tool_calls.async"),
        (json!({"async":"false"}), "tool_calls.async"),
        (
            json!({"caller":{"type":"program","caller_id":"program_synthetic"}}),
            "tool_calls.caller",
        ),
        (
            json!({"caller":{"type":"direct","execution":"synthetic"}}),
            "tool_calls.caller",
        ),
        (
            json!({"namespace":"different_namespace"}),
            "tool_calls.namespace",
        ),
    ] {
        let mut call = tool_call();
        call.as_object_mut()
            .unwrap()
            .extend(extension.as_object().unwrap().clone());
        for client in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
            let request = tool_request(client);
            let prepared = prepare(client, WireApi::Responses, &request, false);
            let error = prepared
                .translate_response_bytes(&serde_json::to_vec(&tool_response(&call)).unwrap())
                .unwrap_err();
            assert_eq!(error.parameter(), Some(parameter));

            for metadata_stage in [1, 5, 6] {
                let mut events = tool_events(&call);
                if metadata_stage > 1 {
                    events[1] = tool_events(&tool_call())[1].clone();
                }
                if metadata_stage > 5 {
                    events[5] = tool_events(&tool_call())[5].clone();
                }
                let mut bridge = prepare(client, WireApi::Responses, &request, true)
                    .into_stream_bridge()
                    .unwrap();
                let frames = events
                    .into_iter()
                    .map(|event| format!("data: {event}\n\n"))
                    .collect::<String>();
                for chunk in frames.as_bytes().chunks(3) {
                    bridge.push(chunk);
                }
                bridge.finish();
                assert!(bridge.completed().is_none(), "{client:?}: {extension}");
                let wire = String::from_utf8(
                    std::iter::from_fn(|| bridge.pop_output())
                        .flatten()
                        .collect(),
                )
                .unwrap();
                assert!(wire.contains(parameter));
                if metadata_stage == 1 {
                    assert!(!wire.contains("call_synthetic_execution"));
                }
            }
        }
    }
}

#[test]
fn responses_history_accepts_only_equivalent_direct_execution_metadata() {
    for custom in [false, true] {
        let mut call = tool_call();
        if custom {
            call["type"] = "custom_tool_call".into();
            call.as_object_mut().unwrap().remove("arguments");
            call["input"] = "synthetic raw input".into();
        }
        let input = |call: &Value| {
            json!([
                {"role":"user","content":"Synthetic request"}, call,
                {"type":if custom { "custom_tool_call_output" } else { "function_call_output" },
                "call_id":"call_synthetic_execution","output":"Synthetic result","caller":{"type":"direct"}}
            ])
        };
        call["async"] = false.into();
        call["caller"] = json!({"type":"direct"});
        let mut decoded = decode::responses_history(&input(&call)).unwrap();
        decode::resolve_tool_history(&mut decoded).unwrap();
        let mut empty_result = input(&call);
        empty_result[2]["output"] = json!([]);
        let decoded = decode::responses_history(&empty_result).unwrap();
        assert!(matches!(&decoded.last().unwrap().blocks[0],
            Block::ToolResult { content, .. } if content.is_empty()));
        if custom {
            for output in [
                json!(42),
                json!([42]),
                json!(["untyped text"]),
                json!([{"text":"untyped content"}]),
            ] {
                let mut malformed = input(&call);
                malformed[2]["output"] = output;
                assert_eq!(
                    decode::responses_history(&malformed)
                        .unwrap_err()
                        .parameter(),
                    Some("input.output")
                );
            }
        }
        let mut unsupported_result = input(&call);
        unsupported_result[2]["caller"] = json!({"type":"program","caller_id":"program_synthetic"});
        assert_eq!(
            decode::responses_history(&unsupported_result)
                .unwrap_err()
                .parameter(),
            Some("input.caller")
        );
        for (extension, parameter) in [
            (json!({"async":true}), "input.async"),
            (
                json!({"caller":{"type":"program","caller_id":"program_synthetic"}}),
                "input.caller",
            ),
        ] {
            let mut unsupported = call.clone();
            unsupported
                .as_object_mut()
                .unwrap()
                .extend(extension.as_object().unwrap().clone());
            assert_eq!(
                decode::responses_history(&input(&unsupported))
                    .unwrap_err()
                    .parameter(),
                Some(parameter)
            );
        }
    }
}

#[test]
fn native_responses_do_not_strip_extended_tool_execution() {
    let mut call = tool_call();
    call["async"] = true.into();
    call["namespace"] = "synthetic_namespace".into();
    call["caller"] = json!({"type":"program","caller_id":"program_synthetic"});
    let request = json!({"model":"test","input":[call],
        "tools":[{"type":"function","name":"lookup","async":true,"parameters":{"type":"object"}}]});
    let prepared = prepare(WireApi::Responses, WireApi::Responses, &request, false);
    assert!(prepared.is_passthrough());
    assert_eq!(prepared.upstream_body()["input"], request["input"]);
    assert_eq!(prepared.upstream_body()["tools"], request["tools"]);
}

#[test]
fn responses_direct_tools_restore_native_items_and_reasoning_after_results() {
    let reasoning = json!({"type":"reasoning","id":"rs_synthetic","summary":[],"encrypted_content":"synthetic-opaque-state"});
    let mut call = tool_call();
    call["async"] = false.into();
    call["caller"] = json!({"type":"direct"});
    call["namespace"] = Value::Null;
    let mut response = tool_response(&call);
    response["output"]
        .as_array_mut()
        .unwrap()
        .push(reasoning.clone());
    for client in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        for streaming in [false, true] {
            let request = tool_request(client);
            let first = prepare(client, WireApi::Responses, &request, streaming);
            let completed = if streaming {
                let mut events = tool_events(&call);
                events.pop();
                events.push(json!({"type":"response.output_item.added","output_index":1,"item":{"type":"reasoning","id":"rs_synthetic","summary":[]}}));
                events.push(
                    json!({"type":"response.output_item.done","output_index":1,"item":reasoning}),
                );
                events.push(json!({"type":"response.completed","response":response}));
                let frames = events
                    .iter()
                    .map(|event| format!("data: {event}\n\n"))
                    .collect::<String>();
                let mut bridge = first.into_stream_bridge().unwrap();
                for chunk in frames.as_bytes().chunks(1) {
                    bridge.push(chunk);
                }
                bridge.finish();
                bridge.completed().unwrap().clone()
            } else {
                first
                    .translate_response_bytes(&serde_json::to_vec(&response).unwrap())
                    .unwrap()
                    .unwrap()
                    .translated_response()
                    .clone()
            };
            let body = &completed.response_body;
            let mut next = request;
            match client {
                WireApi::ChatCompletions => {
                    let messages = next["messages"].as_array_mut().unwrap();
                    messages.push(body["choices"][0]["message"].clone());
                    messages.push(json!({"role":"tool","tool_call_id":call["call_id"],"content":"Файл найден"}));
                }
                WireApi::Messages => {
                    let messages = next["messages"].as_array_mut().unwrap();
                    messages.push(json!({"role":"assistant","content":body["content"]}));
                    messages.push(json!({"role":"user","content":[{"type":"tool_result","tool_use_id":call["call_id"],"content":"Файл найден"}]}));
                }
                WireApi::Gemini => {
                    let contents = next["contents"].as_array_mut().unwrap();
                    contents.push(body["candidates"][0]["content"].clone());
                    contents.push(json!({"role":"user","parts":[{"functionResponse":{"id":call["call_id"],"name":"lookup","response":{"output":"Файл найден"}}}]}));
                }
                _ => unreachable!(),
            }
            let continued = SourceAdapter::between(client, WireApi::Responses)
                .unwrap()
                .prepare_request(AdapterRequestContext {
                    client_wire_api: client,
                    request: &next,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: Some(completed.continuation),
                    response_scope: "source-test",
                    response_id_seed: "continued",
                })
                .unwrap();
            let history = continued.upstream_body()["input"].as_array().unwrap();
            assert!(history.contains(&call));
            assert!(history.contains(&reasoning));
            let expected_output = if client == WireApi::Gemini {
                json!({"output":"Файл найден"}).to_string()
            } else {
                "Файл найден".into()
            };
            assert!(history
                .iter()
                .any(|item| item["type"] == "function_call_output"
                    && item["call_id"] == call["call_id"]
                    && item["output"] == expected_output));
            continued
                .translate_response_bytes(&serde_json::to_vec(&output(WireApi::Responses)).unwrap())
                .unwrap()
                .unwrap();
        }
    }
}
