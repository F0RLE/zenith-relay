use super::*;

fn count_request() -> Value {
    json!({
        "input": "Count synthetic entries",
        "tools": [{
            "type": "function",
            "name": "count_entries",
            "parameters": {
                "type": "object",
                "properties": {"count": {"type": "number", "minimum": 1}},
                "required": ["count"],
                "additionalProperties": false
            }
        }]
    })
}

#[derive(serde::Deserialize)]
struct CountArguments {
    count: usize,
}

#[test]
fn converted_function_arguments_accept_exact_integral_decimal_values() {
    let request = count_request();
    let prepared = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &request,
        false,
    );
    assert_eq!(
        prepared.upstream_body()["tools"][0]["function"]["parameters"],
        request["tools"][0]["parameters"]
    );
    let translated = prepared
        .translate_response_bytes(
            &serde_json::to_vec(&json!({
                "id": "chat_count",
                "choices": [{
                    "index": 0,
                    "message": {
                        "role": "assistant",
                        "tool_calls": [{
                            "id": "call_count",
                            "type": "function",
                            "function": {"name": "count_entries", "arguments": "{\"count\":1000.0}"}
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }))
            .unwrap(),
        )
        .unwrap()
        .unwrap();
    let call = &translated.response_body()["output"][0];
    assert_eq!(call["call_id"], "call_count");
    let arguments = call["arguments"].as_str().unwrap();
    assert_eq!(
        serde_json::from_str::<CountArguments>(arguments)
            .unwrap()
            .count,
        1000
    );
    assert_eq!(arguments, "{\"count\":1000}");
    let continuation = translated.continuation().unwrap().1;
    assert!(matches!(
        &continuation.portable_history.as_ref().unwrap().last().unwrap().blocks[0],
        Block::ToolCall { arguments, .. } if arguments == "{\"count\":1000}"
    ));
}

#[test]
fn fragmented_integral_arguments_match_stream_deltas_and_completed_call() {
    let mut bridge = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &count_request(),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    for (index, fragment) in ["{\"count\":1", "000.", "0}"].into_iter().enumerate() {
        let function = if index == 0 {
            json!({"name": "count_entries", "arguments": fragment})
        } else {
            json!({"arguments": fragment})
        };
        bridge.push(
            format!(
                "data: {}\n\n",
                json!({
                    "id": "chat_count",
                    "choices": [{"index":0, "delta":{"tool_calls":[{
                        "index": 0, "id": "call_count", "type": "function", "function": function
                    }]}}]
                })
            )
            .as_bytes(),
        );
    }
    bridge.push(b"data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n");
    bridge.finish();
    let completed = &bridge.completed().unwrap().response_body;
    let arguments = completed["output"][0]["arguments"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        serde_json::from_str::<CountArguments>(&arguments)
            .unwrap()
            .count,
        1000
    );
    let mut deltas = String::new();
    while let Some(frame) = bridge.pop_output() {
        let text = String::from_utf8(frame).unwrap();
        for line in text.lines().filter_map(|line| line.strip_prefix("data: ")) {
            let event: Value = serde_json::from_str(line).unwrap();
            if event["type"] == "response.function_call_arguments.delta" {
                deltas.push_str(event["delta"].as_str().unwrap());
            }
            if event["type"] == "response.function_call_arguments.done" {
                assert_eq!(event["arguments"], arguments);
            }
        }
    }
    assert_eq!(deltas, arguments);
}

#[test]
fn messages_truncated_tool_stream_keeps_limit_usage_and_is_not_reusable() {
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        for reason in ["max_tokens", "tool_use"] {
            let mut request = input(client);
            request["tools"] = if client == WireApi::Responses {
                count_request()["tools"].clone()
            } else {
                json!([{"type":"function","function":{
                    "name":"count_entries","parameters":{"type":"object","properties":{}}
                }}])
            };
            if client == WireApi::ChatCompletions {
                request["stream_options"] = json!({"include_usage":true});
            }
            let mut bridge = prepare(client, WireApi::Messages, &request, true)
                .into_stream_bridge()
                .unwrap();
            for event in [
                json!({"type":"message_start","message":{"id":"msg_limit","role":"assistant","content":[],"usage":{"input_tokens":3}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_count","name":"count_entries","input":{}}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"count\":1"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":2}}),
                json!({"type":"message_stop"}),
            ] {
                bridge.push(format!("data: {event}\n\n").as_bytes());
            }
            bridge.finish();
            if reason == "tool_use" {
                assert!(bridge.completed().is_none(), "{client:?}");
                continue;
            }
            let completed = bridge
                .completed()
                .expect("token limit is not a corrupt stream");
            assert!(!completed.continuation.reusable);
            assert!(completed.continuation.history_key.is_none());
            let body = &completed.response_body;
            if client == WireApi::Responses {
                assert_eq!(body["status"], "incomplete");
                assert_eq!(body["incomplete_details"]["reason"], "max_output_tokens");
                assert_eq!(body["output"][0]["status"], "incomplete");
                assert_eq!(body["output"][0]["arguments"], "{\"count\":1");
                assert_eq!(body["usage"]["output_tokens"], 2);
            } else {
                assert_eq!(body["choices"][0]["finish_reason"], "length");
                assert_eq!(
                    body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
                    "{\"count\":1"
                );
                assert_eq!(body["usage"]["completion_tokens"], 2);
            }
            let wire = String::from_utf8(
                std::iter::from_fn(|| bridge.pop_output())
                    .flatten()
                    .collect(),
            )
            .unwrap();
            assert!(!wire.contains("response.failed"), "{wire}");
            assert!(!wire.contains("adapter_upstream_stream_invalid"), "{wire}");
            if client == WireApi::Responses {
                assert!(wire.contains("event: response.incomplete"), "{wire}");
            } else {
                assert!(wire.contains("\"choices\":[]"), "{wire}");
                assert!(wire.contains("[DONE]"), "{wire}");
            }
        }
    }
}

fn limited_tool_response(protocol: WireApi, arguments: &str, limited: bool) -> Value {
    if protocol == WireApi::Responses {
        json!({"id":"resp_limit","status":if limited {"incomplete"} else {"completed"},
            "incomplete_details":if limited {json!({"reason":"max_output_tokens"})} else {Value::Null},
            "output":[{"type":"function_call","id":"fc_limit","call_id":"call_limit","name":"lookup",
                "arguments":arguments,"status":"incomplete"}],"usage":{"input_tokens":3,"output_tokens":2}})
    } else {
        json!({"id":"chat_limit","choices":[{"index":0,"message":{"role":"assistant","tool_calls":[{
            "id":"call_limit","type":"function","function":{"name":"lookup","arguments":arguments}
        }]},"finish_reason":if limited {"length"} else {"tool_calls"}}],
            "usage":{"prompt_tokens":3,"completion_tokens":2}})
    }
}

#[test]
fn truncated_tool_json_requires_a_limit_and_a_client_with_string_arguments() {
    for upstream in [WireApi::Responses, WireApi::ChatCompletions] {
        for client in WireApi::ALL
            .into_iter()
            .filter(|client| *client != upstream)
        {
            let mut request = input(client);
            request["tools"] = tool_history(client)["tools"].clone();
            for (arguments, truncated_object) in [
                ("", true),
                (" ", true),
                ("{", true),
                ("{\"value\":1", true),
                ("{\"value\":]", false),
                ("[", false),
            ] {
                for limited in [true, false] {
                    let result = prepare(client, upstream, &request, false)
                        .translate_response_bytes(
                            &serde_json::to_vec(&limited_tool_response(
                                upstream, arguments, limited,
                            ))
                            .unwrap(),
                        );
                    let representable =
                        matches!(client, WireApi::Responses | WireApi::ChatCompletions);
                    assert_eq!(
                        result.is_ok(),
                        truncated_object && limited && representable,
                        "{client:?} <- {upstream:?}: {arguments}, limited={limited}"
                    );
                    if let Ok(Some(result)) = result {
                        let body = result.response_body();
                        assert_eq!(
                            if client == WireApi::Responses {
                                &body["output"][0]["arguments"]
                            } else {
                                &body["choices"][0]["message"]["tool_calls"][0]["function"]
                                    ["arguments"]
                            },
                            arguments
                        );
                        assert!(result.continuation().is_none());
                    }
                }
            }
        }
    }
}

#[test]
fn openai_truncated_tool_streams_keep_their_incomplete_terminal() {
    for upstream in [WireApi::Responses, WireApi::ChatCompletions] {
        let client = if upstream == WireApi::Responses {
            WireApi::ChatCompletions
        } else {
            WireApi::Responses
        };
        let mut request = input(client);
        request["tools"] = tool_history(client)["tools"].clone();
        for (arguments, limited) in [
            ("{\"value\":1", true),
            ("", true),
            ("{\"value\":1", false),
            ("", false),
        ] {
            let response = limited_tool_response(upstream, arguments, limited);
            let events = if upstream == WireApi::Responses {
                let mut start = response["output"][0].clone();
                start["status"] = "in_progress".into();
                start["arguments"] = "".into();
                vec![
                    json!({"type":"response.output_item.added","output_index":0,"item":start}),
                    json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_limit","delta":arguments}),
                    json!({"type":"response.function_call_arguments.done","output_index":0,"item_id":"fc_limit","arguments":arguments}),
                    json!({"type":"response.output_item.done","output_index":0,"item":response["output"][0]}),
                    json!({"type":if limited {"response.incomplete"} else {"response.completed"},"response":response}),
                ]
            } else {
                vec![
                    json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_limit","type":"function","function":{"name":"lookup","arguments":arguments}}]}}]}),
                    json!({"choices":[{"index":0,"delta":{},"finish_reason":if limited {"length"} else {"tool_calls"}}],"usage":{"prompt_tokens":3,"completion_tokens":2}}),
                ]
            };
            let mut bridge = prepare(client, upstream, &request, true)
                .into_stream_bridge()
                .unwrap();
            for event in events {
                bridge.push(format!("data: {event}\n\n").as_bytes());
            }
            if upstream == WireApi::ChatCompletions {
                bridge.push(b"data: [DONE]\n\n");
            }
            bridge.finish();
            assert_eq!(bridge.completed().is_some(), limited, "{upstream:?}");
            if let Some(completed) = bridge.completed() {
                assert!(!completed.continuation.reusable);
                assert!(completed.continuation.history_key.is_none());
                let wire = String::from_utf8(
                    std::iter::from_fn(|| bridge.pop_output())
                        .flatten()
                        .collect(),
                )
                .unwrap();
                assert!(wire.contains("call_limit"), "{wire}");
                if client == WireApi::Responses {
                    assert_eq!(
                        wire.matches("event: response.output_item.added\n").count(),
                        1
                    );
                    assert_eq!(
                        wire.matches("event: response.output_item.done\n").count(),
                        1
                    );
                }
            }
        }
    }
}

#[test]
fn messages_without_argument_deltas_distinguish_empty_object_from_output_limit() {
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        let mut request = input(client);
        request["tools"] = tool_history(client)["tools"].clone();
        for limited in [true, false] {
            let mut bridge = prepare(client, WireApi::Messages, &request, true)
                .into_stream_bridge()
                .unwrap();
            for event in [
                json!({"type":"message_start","message":{"id":"msg_limit","usage":{"input_tokens":3}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_limit","name":"lookup","input":{}}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"message_delta","delta":{"stop_reason":if limited {"max_tokens"} else {"tool_use"}},"usage":{"output_tokens":2}}),
                json!({"type":"message_stop"}),
            ] {
                bridge.push(format!("data: {event}\n\n").as_bytes());
            }
            bridge.finish();
            let completed = bridge
                .completed()
                .expect("terminal without argument deltas");
            let body = &completed.response_body;
            let arguments = if client == WireApi::Responses {
                &body["output"][0]["arguments"]
            } else {
                &body["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"]
            };
            assert_eq!(arguments, if limited { "" } else { "{}" });
            assert_eq!(completed.continuation.reusable, !limited);
        }
    }
}
