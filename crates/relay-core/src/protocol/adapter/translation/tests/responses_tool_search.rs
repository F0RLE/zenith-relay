use super::*;

fn catalog() -> Value {
    json!({"input":"Find the record","parallel_tool_calls":false,"tools":[
        {"type":"tool_search","execution":"client","parameters":{"type":"object","properties":{"goal":{"type":"string"}},"required":["goal"],"additionalProperties":false}},
        {"type":"namespace","name":"records","tools":[{"type":"function","name":"lookup","defer_loading":true,
            "parameters":{"type":"object","properties":{"id":{"type":"integer","description":"synthetic-original-schema"}},"required":["id"],"additionalProperties":false}}]},
        {"type":"function","name":"unused","defer_loading":true,"parameters":{"type":"object","description":"synthetic-never-loaded"}}
    ]})
}

fn prepare_search(
    upstream: WireApi,
    body: &Value,
    stream: bool,
    previous: Option<crate::AdapterContinuationState>,
    seed: &str,
) -> crate::PreparedAdapterRequest {
    SourceAdapter::between(WireApi::Responses, upstream)
        .unwrap()
        .prepare_request_with_capabilities(
            AdapterRequestContext {
                client_wire_api: WireApi::Responses,
                request: body,
                model: "test",
                stream,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous,
                response_scope: "client-tool-search",
                response_id_seed: seed,
            },
            &crate::model_metadata::ModelCapabilities {
                reasoning: Some(false),
                ..Default::default()
            },
        )
        .unwrap_or_else(|error| panic!("{upstream:?}: {error}"))
}

fn callable(body: &Value, upstream: WireApi) -> String {
    let name = match upstream {
        WireApi::ChatCompletions => &body["tools"][0]["function"]["name"],
        WireApi::Messages => &body["tools"][0]["name"],
        WireApi::Gemini => &body["tools"][0]["functionDeclarations"][0]["name"],
        _ => unreachable!(),
    };
    name.as_str().unwrap().into()
}

fn call_reply(upstream: WireApi, id: &str, name: &str, args: &Value) -> Value {
    match upstream {
        WireApi::ChatCompletions => {
            json!({"id":"chat_synthetic","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[{"type":"function","id":id,"function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":"tool_calls"}]})
        }
        WireApi::Messages => {
            json!({"id":"msg_synthetic","content":[{"type":"tool_use","id":id,"name":name,"input":args}],"stop_reason":"tool_use"})
        }
        WireApi::Gemini => {
            json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"functionCall":{"id":id,"name":name,"args":args}}]},"finishReason":"STOP"}]})
        }
        _ => unreachable!(),
    }
}

fn call_events(upstream: WireApi, id: &str, name: &str, args: &Value) -> Vec<Value> {
    match upstream {
        WireApi::ChatCompletions => vec![
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"type":"function","id":id,"function":{"name":name,"arguments":args.to_string()}}]},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ],
        WireApi::Messages => vec![
            json!({"type":"message_start","message":{"id":"msg_synthetic","usage":{"input_tokens":3}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":id,"name":name,"input":{}}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":args.to_string()}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ],
        WireApi::Gemini => vec![call_reply(upstream, id, name, args)],
        _ => unreachable!(),
    }
}

fn complete_call(
    prepared: crate::PreparedAdapterRequest,
    upstream: WireApi,
    streaming: bool,
    id: &str,
    name: &str,
    args: &Value,
    search: bool,
) -> crate::TranslatedResponse {
    if !streaming {
        let reply = call_reply(upstream, id, name, args);
        return prepared
            .translate_response_bytes(&serde_json::to_vec(&reply).unwrap())
            .unwrap()
            .unwrap()
            .translated_response()
            .clone();
    }
    let mut events = call_events(upstream, id, name, args);
    let last = events.pop().unwrap();
    let mut bridge = prepared.into_stream_bridge().unwrap();
    for event in events {
        for bytes in format!("data: {event}\n\n").as_bytes().chunks(3) {
            bridge.push(bytes);
        }
        if search {
            let wire = String::from_utf8(
                std::iter::from_fn(|| bridge.pop_output())
                    .flatten()
                    .collect(),
            )
            .unwrap();
            assert!(!wire.contains("\"type\":\"tool_search_call\""));
        }
    }
    bridge.push(format!("data: {last}\n\n").as_bytes());
    if upstream == WireApi::ChatCompletions {
        bridge.push(b"data: [DONE]\n\n");
    }
    bridge.finish();
    bridge
        .completed()
        .unwrap_or_else(|| panic!("{upstream:?}: stream incomplete"))
        .clone()
}

#[test]
fn client_tool_search_completes_discovery_call_result_and_next_answer_across_json_and_sse() {
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        for streaming in [false, true] {
            for by_id in [false, true] {
                let original = catalog();
                let first = prepare_search(upstream, &original, streaming, None, "search");
                assert!(
                    !first
                        .upstream_body()
                        .to_string()
                        .contains("synthetic-original-schema")
                );
                assert!(
                    !first
                        .upstream_body()
                        .to_string()
                        .contains("synthetic-never-loaded")
                );
                let name = callable(first.upstream_body(), upstream);
                let first = complete_call(
                    first,
                    upstream,
                    streaming,
                    "call_search",
                    &name,
                    &json!({"goal":"lookup"}),
                    true,
                );
                let item = &first.response_body["output"][0];
                assert_eq!(item["type"], "tool_search_call");
                assert_eq!(item["execution"], "client");
                assert_eq!(item["call_id"], "call_search");
                assert_eq!(item["arguments"], json!({"goal":"lookup"}));
                let result = json!({"type":"tool_search_output","execution":"client","status":"completed","call_id":"call_search","tools":[original["tools"][1]]});
                let second = if by_id {
                    json!({"previous_response_id":first.response_id,"input":[result]})
                } else {
                    json!({"tools":original["tools"],"input":[{"role":"user","content":"Find the record"},item,result]})
                };
                let next = prepare_search(
                    upstream,
                    &second,
                    streaming,
                    Some(first.continuation),
                    "lookup",
                );
                let wire = next.upstream_body().to_string();
                assert_eq!(
                    wire.matches("synthetic-original-schema").count(),
                    1,
                    "{upstream:?}: {wire}"
                );
                assert!(!wire.contains("synthetic-never-loaded"));
                let lookup = if by_id {
                    callable(next.upstream_body(), upstream)
                } else {
                    let decoded = decode::request(upstream, next.upstream_body()).unwrap();
                    decoded
                        .tools
                        .iter()
                        .find(|tool| tool.parameters["properties"].get("id").is_some())
                        .unwrap()
                        .name
                        .clone()
                };
                let next = complete_call(
                    next,
                    upstream,
                    streaming,
                    "call_lookup",
                    &lookup,
                    &json!({"id":7}),
                    false,
                );
                assert_eq!(next.response_body["output"][0]["type"], "function_call");
                assert_eq!(next.response_body["output"][0]["name"], "lookup");
                assert_eq!(next.response_body["output"][0]["namespace"], "records");
                let final_request = json!({"previous_response_id":next.response_id,"input":[{"type":"function_call_output","call_id":"call_lookup","output":"ok"}]});
                let final_prepared = prepare_search(
                    upstream,
                    &final_request,
                    streaming,
                    Some(next.continuation),
                    "answer",
                );
                let final_response = if streaming {
                    let mut bridge = final_prepared.into_stream_bridge().unwrap();
                    for bytes in sse(upstream).chunks(3) {
                        bridge.push(bytes);
                    }
                    bridge.finish();
                    bridge.completed().unwrap().clone()
                } else {
                    final_prepared
                        .translate_response_bytes(&serde_json::to_vec(&output(upstream)).unwrap())
                        .unwrap()
                        .unwrap()
                        .translated_response()
                        .clone()
                };
                assert_eq!(final_response.response_body["status"], "completed");
                assert!(final_response.response_body.to_string().contains("Hi"));
            }
        }
    }
}

#[test]
fn removed_deferred_tools_stay_in_history_without_becoming_callable_again() {
    let mut request = catalog();
    request["input"] = json!([
        {"role":"user","content":"Find the record"},
        {"type":"tool_search_call","execution":"client","call_id":"old_search","status":"completed","arguments":{"goal":"lookup"}},
        {"type":"tool_search_output","execution":"client","call_id":"old_search","status":"completed","tools":[]},
        {"type":"function_call","call_id":"old_lookup","name":"lookup","namespace":"records","arguments":"{\"id\":7}"},
        {"type":"function_call_output","call_id":"old_lookup","output":"Previous result"},
        {"role":"user","content":"Continue without the removed tool"}
    ]);
    let alias = ClientToolTarget::from_definition(
        request["tools"][1]["tools"][0].as_object().unwrap(),
        Some("records"),
    )
    .unwrap()
    .upstream_name();
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        for streaming in [false, true] {
            let prepared = prepare_search(upstream, &request, streaming, None, "revoked");
            let decoded = decode::request(upstream, prepared.upstream_body()).unwrap();
            assert!(
                !decoded.tools.iter().any(|tool| tool.name == alias),
                "{upstream:?}"
            );
            assert!(decoded.messages.iter().flat_map(|message| &message.blocks).any(|block| {
                matches!(block, Block::ToolCall { id, name, .. } if id == "old_lookup" && name == &alias)
            }));
            let arguments = json!({"id":8});
            if streaming {
                let mut bridge = prepared.into_stream_bridge().unwrap();
                for event in call_events(upstream, "new_lookup", &alias, &arguments) {
                    for bytes in format!("data: {event}\n\n").as_bytes().chunks(3) {
                        bridge.push(bytes);
                    }
                }
                if upstream == WireApi::ChatCompletions {
                    bridge.push(b"data: [DONE]\n\n");
                }
                bridge.finish();
                assert!(bridge.completed().is_none());
                let wire = String::from_utf8(
                    std::iter::from_fn(|| bridge.pop_output())
                        .flatten()
                        .collect(),
                )
                .unwrap();
                assert!(wire.contains("tool_calls.function.name"));
                assert!(!wire.contains("new_lookup"));
            } else {
                let reply = call_reply(upstream, "new_lookup", &alias, &arguments);
                let error = prepared
                    .translate_response_bytes(&serde_json::to_vec(&reply).unwrap())
                    .unwrap_err();
                assert_eq!(error.parameter(), Some("tool_calls.function.name"));
            }

            let mut explicit = request.clone();
            explicit["tool_choice"] =
                json!({"type":"function","name":"lookup","namespace":"records"});
            let prepared = prepare_search(upstream, &explicit, streaming, None, "explicit");
            let decoded = decode::request(upstream, prepared.upstream_body()).unwrap();
            assert_eq!(decoded.tools.len(), 1);
            assert_eq!(decoded.tools[0].name, alias);
            let response = complete_call(
                prepared,
                upstream,
                streaming,
                "explicit_lookup",
                &alias,
                &arguments,
                false,
            );
            assert_eq!(response.response_body["output"][0]["name"], "lookup");
            assert_eq!(response.response_body["output"][0]["namespace"], "records");
        }
    }
}

#[test]
fn client_tool_search_rejects_wrong_result_kinds_and_unpaired_results() {
    let mut request = catalog();
    request["input"] = json!([
        {"role":"user","content":"Find the record"},
        {"type":"tool_search_call","execution":"client","call_id":"search","status":"completed","arguments":{"goal":"lookup"}},
        {"type":"function_call_output","call_id":"search","output":"not a search result"}
    ]);
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        assert!(try_convert(WireApi::Responses, upstream, &request, false).is_err());
    }
    request["input"] = json!([{ "type":"tool_search_output","execution":"client","call_id":"orphan","status":"completed","tools":[] }]);
    assert!(
        try_convert(
            WireApi::Responses,
            WireApi::ChatCompletions,
            &request,
            false
        )
        .is_err()
    );
}
