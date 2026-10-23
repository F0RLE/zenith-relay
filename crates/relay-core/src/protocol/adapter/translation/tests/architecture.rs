use super::*;
use crate::PreparedAdapterRequest;

fn try_prepare(upstream: WireApi, request: &Value) -> AdapterResult<PreparedAdapterRequest> {
    SourceAdapter::between(WireApi::Responses, upstream)
        .unwrap()
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "synthetic-architecture",
            response_id_seed: "first",
        })
}

#[test]
fn exactly_four_native_and_twelve_shared_execution_paths() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL {
            let prepared = prepare(client, upstream, &input(client), false);
            assert_eq!(
                matches!(prepared, PreparedAdapterRequest::Native { .. }),
                client == upstream
            );
            assert_eq!(
                matches!(prepared, PreparedAdapterRequest::Translated { .. }),
                client != upstream
            );
        }
    }
}

#[test]
fn missing_continuation_precedes_tool_result_identity_validation() {
    let request = json!({
        "previous_response_id":"resp_synthetic_expired",
        "input":[{"type":"function_call_output","call_id":"synthetic_call","output":"ok"}]
    });
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        assert_eq!(
            try_prepare(upstream, &request).unwrap_err().code(),
            "adapter_continuation_missing",
            "{upstream:?}"
        );
    }
}

#[test]
fn hosted_tools_are_never_silently_dropped_at_any_catalog_depth() {
    for tools in [
        json!([{"type":"web_search"}]),
        json!([{"type":"file_search","vector_store_ids":["vs_synthetic"]}]),
        json!([{"type":"code_interpreter","container":{"type":"auto"}}]),
        json!([{"type":"mcp","server_label":"synthetic","server_url":"https://example.invalid/mcp"}]),
        json!([{"type":"tool_search","execution":"server"}]),
        json!([{"type":"function","name":"lookup","parameters":{"type":"object"}}, {"type":"web_search"}]),
        json!([{"type":"namespace","name":"synthetic","tools":[{"type":"web_search"}]}]),
    ] {
        let request = json!({"input":"Hello","tools":tools});
        for upstream in WireApi::ALL {
            let prepared = try_prepare(upstream, &request);
            if upstream == WireApi::Responses {
                assert_eq!(prepared.unwrap().upstream_body()["tools"], tools);
            } else {
                assert_eq!(
                    prepared.unwrap_err().code(),
                    "adapter_tool_unsupported",
                    "{upstream:?}"
                );
            }
        }
    }
}

#[test]
fn client_tool_discovery_history_requires_paired_calls_and_results() {
    for (item, expected) in [
        (
            json!({"type":"tool_search_call","execution":"client","call_id":"call_search","status":"completed","arguments":{"goal":"Find a synthetic tool"}}),
            "adapter_invalid_request",
        ),
        (
            json!({"type":"tool_search_output","execution":"client","call_id":"call_search","status":"completed","tools":[]}),
            "adapter_continuation_mismatch",
        ),
    ] {
        let request = json!({"input":[item,{"role":"user","content":"Continue"}]});
        for upstream in WireApi::ALL {
            let prepared = try_prepare(upstream, &request);
            if upstream == WireApi::Responses {
                assert_eq!(prepared.unwrap().upstream_body()["input"], request["input"]);
            } else {
                assert_eq!(prepared.unwrap_err().code(), expected);
            }
        }
    }
}

#[test]
fn converted_tools_never_broaden_the_permitted_caller() {
    for callers in [
        json!([]),
        json!(["code_execution"]),
        json!(["direct", "code_execution"]),
    ] {
        let request = json!({"input":"Hello","tools":[{
            "type":"function","name":"lookup","parameters":{"type":"object"},
            "allowed_callers":callers
        }]});
        for upstream in WireApi::ALL {
            let prepared = try_prepare(upstream, &request);
            if upstream == WireApi::Responses {
                assert_eq!(prepared.unwrap().upstream_body()["tools"], request["tools"]);
            } else {
                assert_eq!(prepared.unwrap_err().code(), "adapter_tool_unsupported");
            }
        }
    }
}

#[test]
fn gemini_schema_constraints_are_preserved_or_rejected_before_transport() {
    let schema =
        json!({"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]});
    for upstream in WireApi::ALL {
        let adapter = SourceAdapter::between(WireApi::Gemini, upstream).unwrap();
        for invalid in [
            json!({"generationConfig":{"responseMimeType":"application/json","responseSchema":schema,"responseJsonSchema":schema}}),
            json!({"generationConfig":{"responseSchema":schema}}),
            json!({"generationConfig":{"responseMimeType":"text/plain","responseJsonSchema":schema}}),
            json!({"tools":[{"functionDeclarations":[{"name":"lookup","parameters":schema,"parametersJsonSchema":schema}]}]}),
        ] {
            let mut request = input(WireApi::Gemini);
            request
                .as_object_mut()
                .unwrap()
                .extend(invalid.as_object().unwrap().clone());
            let prepared = adapter.prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Gemini,
                request: &request,
                model: "synthetic-model",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "schema-contract",
                response_id_seed: "schema-contract",
            });
            if upstream == WireApi::Gemini {
                assert!(
                    prepared.is_ok(),
                    "native routes leave provider validation native"
                );
            } else {
                assert_eq!(prepared.unwrap_err().code(), "adapter_invalid_request");
            }
        }
    }
}

#[test]
fn gemini_openapi_tools_and_output_use_the_destination_schema_dialect() {
    let schema = json!({"type":"OBJECT","properties":{"value":{"type":"INTEGER","nullable":true}},"required":["value"]});
    let mut request = input(WireApi::Gemini);
    request["tools"] = json!([{"functionDeclarations":[{"name":"lookup","parameters":schema}]}]);
    request["generationConfig"] =
        json!({"responseMimeType":"application/json","responseSchema":schema});
    for upstream in WireApi::ALL {
        let prepared = prepare(WireApi::Gemini, upstream, &request, false);
        let (tool, output) = match upstream {
            WireApi::Responses => ("/tools/0/parameters", "/text/format/schema"),
            WireApi::ChatCompletions => (
                "/tools/0/function/parameters",
                "/response_format/json_schema/schema",
            ),
            WireApi::Messages => ("/tools/0/input_schema", "/output_config/format/schema"),
            WireApi::Gemini => (
                "/tools/0/functionDeclarations/0/parameters",
                "/generationConfig/responseSchema",
            ),
        };
        for pointer in [tool, output] {
            let converted = prepared.upstream_body().pointer(pointer).unwrap();
            if upstream == WireApi::Gemini {
                assert_eq!(converted, &schema);
            } else {
                assert_eq!(converted["type"], "object");
                assert_eq!(
                    converted["properties"]["value"]["anyOf"],
                    json!([{"type":"integer"},{"type":"null"}])
                );
                assert_eq!(converted["required"], json!(["value"]));
            }
        }
    }
}

#[test]
fn generation_extensions_preserve_values_or_fail_before_transport() {
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let top_k = try_prepare(upstream, &json!({"input":"Hello","top_k":17}));
        if upstream == WireApi::ChatCompletions {
            assert_eq!(top_k.unwrap_err().code(), "adapter_parameter_unsupported");
        } else {
            let body = top_k.unwrap();
            assert_eq!(
                body.upstream_body()
                    .pointer(if upstream == WireApi::Gemini {
                        "/generationConfig/topK"
                    } else {
                        "/top_k"
                    }),
                Some(&json!(17))
            );
        }
        let controls = try_prepare(
            upstream,
            &json!({"input":"Hello","presence_penalty":0.5,"frequency_penalty":-0.25,"seed":-12}),
        );
        if upstream == WireApi::Messages {
            assert_eq!(
                controls.unwrap_err().code(),
                "adapter_parameter_unsupported"
            );
        } else {
            let prepared = controls.unwrap();
            let body = if upstream == WireApi::Gemini {
                &prepared.upstream_body()["generationConfig"]
            } else {
                prepared.upstream_body()
            };
            assert_eq!(body["seed"], -12);
            assert_eq!(
                body[if upstream == WireApi::Gemini {
                    "presencePenalty"
                } else {
                    "presence_penalty"
                }],
                0.5
            );
            assert_eq!(
                body[if upstream == WireApi::Gemini {
                    "frequencyPenalty"
                } else {
                    "frequency_penalty"
                }],
                -0.25
            );
        }
    }
    for invalid in [
        json!({"top_k":0}),
        json!({"top_k":1.5}),
        json!({"seed":0.5}),
    ] {
        let mut request = json!({"input":"Hello"});
        request
            .as_object_mut()
            .unwrap()
            .extend(invalid.as_object().unwrap().clone());
        assert_eq!(
            try_prepare(WireApi::Gemini, &request).unwrap_err().code(),
            "adapter_invalid_request"
        );
    }
}

#[test]
fn structured_output_keeps_envelope_and_schema_descriptions() {
    let format = json!({"type":"json_schema","name":"answer","description":"Envelope description",
        "schema":{"type":"object","description":"Schema description","properties":{"value":{"type":"string"}},"required":["value"],"additionalProperties":false}});
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let prepared =
            try_prepare(upstream, &json!({"input":"Hello","text":{"format":format}})).unwrap();
        let body = prepared.upstream_body();
        if upstream == WireApi::ChatCompletions {
            assert_eq!(
                body["response_format"]["json_schema"]["description"],
                "Envelope description"
            );
            assert_eq!(
                body["response_format"]["json_schema"]["schema"],
                format["schema"]
            );
        } else {
            let schema = body
                .pointer(if upstream == WireApi::Messages {
                    "/output_config/format/schema"
                } else {
                    "/generationConfig/responseJsonSchema"
                })
                .unwrap();
            assert_eq!(
                schema["description"],
                "Envelope description\nSchema description"
            );
            assert_eq!(schema["required"], format["schema"]["required"]);
        }
    }
    let alias = try_prepare(
        WireApi::Gemini,
        &json!({"input":"Hello","response_format":format}),
    )
    .unwrap();
    assert!(alias
        .upstream_body()
        .pointer("/generationConfig/responseJsonSchema")
        .is_some());
    assert!(try_prepare(
        WireApi::Gemini,
        &json!({"input":"Hello","text":{"format":format},"response_format":format})
    )
    .is_err());
}

pub(super) fn tool_reply(protocol: WireApi) -> Value {
    let calls = (0..2).map(|index| json!({"type":"function_call","id":format!("fc_{index}"),"call_id":format!("call_new_{index}"),"name":"lookup","arguments":format!("{{\"value\":{index}}}")})).collect::<Vec<_>>();
    match protocol {
        WireApi::Responses => {
            json!({"id":"resp_tools","status":"completed","output":calls,"usage":{"input_tokens":11,"output_tokens":7}})
        }
        WireApi::ChatCompletions => {
            json!({"id":"chat_tools","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":(0..2).map(|i|json!({"id":format!("call_new_{i}"),"type":"function","function":{"name":"lookup","arguments":format!("{{\"value\":{i}}}")}})).collect::<Vec<_>>()},"finish_reason":"tool_calls"}]})
        }
        WireApi::Messages => {
            json!({"id":"msg_tools","content":(0..2).map(|i|json!({"type":"tool_use","id":format!("call_new_{i}"),"name":"lookup","input":{"value":i}})).collect::<Vec<_>>(),"stop_reason":"tool_use"})
        }
        WireApi::Gemini => {
            json!({"candidates":[{"index":0,"content":{"role":"model","parts":(0..2).map(|i|json!({"functionCall":{"id":format!("call_new_{i}"),"name":"lookup","args":{"value":i}}})).collect::<Vec<_>>()},"finishReason":"STOP"}]})
        }
    }
}

fn with_tool_results(client: WireApi, completed: &TranslatedResponse) -> Value {
    append_tool_results(client, tool_history(client), completed, false)
}

pub(super) fn append_tool_results(
    client: WireApi,
    mut request: Value,
    completed: &TranslatedResponse,
    reverse: bool,
) -> Value {
    let body = &completed.response_body;
    let decoded = response::decode(client, body, "test").unwrap();
    let mut calls = decoded
        .blocks
        .iter()
        .filter_map(|block| {
            if let Block::ToolCall { id, name, .. } = block {
                Some((id, name))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if reverse {
        calls.reverse();
    }
    if client == WireApi::Responses {
        request["previous_response_id"] = completed.response_id.clone().into();
        request["input"] = calls.iter().map(|(id, _)|json!({"type":"function_call_output","call_id":id,"output":"synthetic result"})).collect::<Vec<_>>().into();
        return request;
    }
    let key = if client == WireApi::Gemini {
        "contents"
    } else {
        "messages"
    };
    let messages = request[key].as_array_mut().unwrap();
    messages.push(match client {
        WireApi::ChatCompletions => body["choices"][0]["message"].clone(),
        WireApi::Messages => json!({"role":"assistant","content":body["content"]}),
        WireApi::Gemini => body["candidates"][0]["content"].clone(),
        _ => unreachable!(),
    });
    if client == WireApi::ChatCompletions {
        messages.extend(
            calls.iter().map(
                |(id, _)| json!({"role":"tool","tool_call_id":id,"content":"synthetic result"}),
            ),
        );
    } else if client == WireApi::Messages {
        messages.push(json!({"role":"user","content":calls.iter().map(|(id, _)|json!({"type":"tool_result","tool_use_id":id,"content":"synthetic result"})).collect::<Vec<_>>()}));
    } else {
        messages.push(json!({"role":"user","parts":calls.iter().map(|(id, name)|json!({"functionResponse":{"id":id,"name":name,"response":{"output":"synthetic result"}}})).collect::<Vec<_>>()}));
    }
    request
}

#[test]
fn every_conversion_completes_real_tool_result_cycles_for_json_and_fragmented_sse() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL {
            if client == upstream {
                continue;
            }
            for streaming in [false, true] {
                let first =
                    super::matrix::prepare_portable_tool_history(client, upstream, streaming);
                let completed = if streaming {
                    let mut bridge = first.into_stream_bridge().unwrap();
                    for fragment in tool_stream(upstream).chunks(3) {
                        bridge.push(fragment);
                    }
                    bridge.finish();
                    bridge.completed().expect("tool stream completes").clone()
                } else {
                    first
                        .translate_response_bytes(
                            &serde_json::to_vec(&tool_reply(upstream)).unwrap(),
                        )
                        .unwrap()
                        .unwrap()
                        .translated_response()
                        .clone()
                };
                let request = with_tool_results(client, &completed);
                let continued = SourceAdapter::between(client, upstream)
                    .unwrap()
                    .prepare_request(AdapterRequestContext {
                        client_wire_api: client,
                        request: &request,
                        model: "test",
                        stream: false,
                        reasoning_mode: MessagesReasoningMode::Disabled,
                        cache_write_ttl: CacheWriteTtl::Provider,
                        previous: Some(completed.continuation),
                        response_scope: "source-test",
                        response_id_seed: "next",
                    })
                    .unwrap_or_else(|error| {
                        panic!("{client:?} -> {upstream:?}, streaming={streaming}: {error}")
                    });
                let mut decoded = decode::request(upstream, continued.upstream_body()).unwrap();
                decode::resolve_tool_history(&mut decoded.messages).unwrap();
                for expected in ["call_new_0", "call_new_1"] {
                    assert_eq!(
                        decoded
                            .messages
                            .iter()
                            .flat_map(|message| &message.blocks)
                            .filter(
                                |block| matches!(block, Block::ToolResult{id,..} if id == expected)
                            )
                            .count(),
                        1
                    );
                }
                let final_reply = continued
                    .translate_response_bytes(&serde_json::to_vec(&output(upstream)).unwrap())
                    .unwrap()
                    .unwrap();
                assert!(final_reply.continuation().is_some());
            }
        }
    }
}

#[test]
fn custom_result_text_and_kind_survive_catalog_replacement_on_all_upstreams() {
    for upstream in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        let first = try_prepare(
            upstream,
            &json!({"input":"Hello","tools":[{"type":"custom","name":"lookup"}]}),
        )
        .unwrap();
        let mut reply = tool_reply(upstream);
        match upstream {
            WireApi::ChatCompletions => {
                for call in reply["choices"][0]["message"]["tool_calls"]
                    .as_array_mut()
                    .unwrap()
                {
                    call["function"]["arguments"] = "{\"input\":\"synthetic\"}".into();
                }
            }
            WireApi::Messages => {
                for call in reply["content"].as_array_mut().unwrap() {
                    call["input"] = json!({"input":"synthetic"});
                }
            }
            WireApi::Gemini => {
                for call in reply["candidates"][0]["content"]["parts"]
                    .as_array_mut()
                    .unwrap()
                {
                    call["functionCall"]["args"] = json!({"input":"synthetic"});
                }
            }
            _ => unreachable!(),
        }
        let completed = first
            .translate_response_bytes(&serde_json::to_vec(&reply).unwrap())
            .unwrap()
            .unwrap();
        let text = "{\"result\":\"literal JSON-looking text\"}";
        let make_request = |kind: &str, output: &Value| json!({"previous_response_id":completed.response_id(),"input":(0..2).map(|i|json!({"type":kind,"call_id":format!("call_new_{i}"),"output":output})).collect::<Vec<_>>(),"tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}]});
        let resume = |request: &Value| {
            SourceAdapter::between(WireApi::Responses, upstream)
                .unwrap()
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Responses,
                    request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: Some(completed.continuation().unwrap().1.clone()),
                    response_scope: "synthetic-architecture",
                    response_id_seed: "next",
                })
        };
        assert_eq!(
            resume(&make_request("function_call_output", &json!(text)))
                .unwrap_err()
                .code(),
            "adapter_continuation_mismatch"
        );
        for output in [json!(text), json!([{"type":"input_text","text":text}])] {
            let continued = resume(&make_request("custom_tool_call_output", &output))
                .unwrap_or_else(|error| panic!("{upstream:?}, output={output}: {error}"));
            let mut decoded = decode::request(upstream, continued.upstream_body()).unwrap();
            decode::resolve_tool_history(&mut decoded.messages).unwrap();
            let expected_content = vec![Block::Text(if upstream == WireApi::Gemini {
                json!({"output":text}).to_string()
            } else {
                text.into()
            })];
            assert_eq!(
                decoded
                    .messages
                    .iter()
                    .flat_map(|message| &message.blocks)
                    .filter(|block| matches!(block, Block::ToolResult { content, .. }
                        if content == &expected_content))
                    .count(),
                2
            );
            if upstream == WireApi::Gemini {
                let contents = continued.upstream_body()["contents"].as_array().unwrap();
                assert_eq!(
                    contents.last().unwrap()["parts"][0]["functionResponse"]["response"],
                    json!({"output":text})
                );
            }
            continued
                .translate_response_bytes(&serde_json::to_vec(&super::output(upstream)).unwrap())
                .unwrap()
                .unwrap();
        }
        let empty_request = make_request("custom_tool_call_output", &json!([]));
        let empty = resume(&empty_request).unwrap();
        let body = empty.upstream_body();
        let empty_content = match upstream {
            WireApi::ChatCompletions => {
                &body["messages"].as_array().unwrap().last().unwrap()["content"]
            }
            WireApi::Messages => {
                &body["messages"].as_array().unwrap().last().unwrap()["content"][0]["content"]
            }
            WireApi::Gemini => &body["contents"].as_array().unwrap().last().unwrap()["parts"][0]
                ["functionResponse"]["response"]["output"],
            _ => unreachable!(),
        };
        assert_eq!(empty_content, "");
        empty
            .translate_response_bytes(&serde_json::to_vec(&super::output(upstream)).unwrap())
            .unwrap()
            .unwrap();
        let media_output = json!([
            {"type":"input_text","text":"Synthetic image"},
            {"type":"input_image","image_url":"data:image/png;base64,YQ=="}
        ]);
        let media_request = make_request("custom_tool_call_output", &media_output);
        if upstream == WireApi::ChatCompletions {
            assert_eq!(
                resume(&media_request).unwrap_err().parameter(),
                Some("input.output")
            );
        } else {
            let continued = resume(&media_request).unwrap();
            let decoded = decode::request(upstream, continued.upstream_body()).unwrap();
            assert_eq!(
                decoded
                    .messages
                    .iter()
                    .flat_map(|message| &message.blocks)
                    .filter(|block| matches!(block, Block::ToolResult { content, .. }
                    if matches!(content.as_slice(), [Block::Text(_), Block::Image { .. }])))
                    .count(),
                2
            );
            continued
                .translate_response_bytes(&serde_json::to_vec(&super::output(upstream)).unwrap())
                .unwrap()
                .unwrap();
        }
        for invalid in [
            json!(null),
            json!(true),
            json!(42),
            json!({"result":"not a content list"}),
            json!([{"type":"unknown_content"}]),
        ] {
            assert!(resume(&make_request("custom_tool_call_output", &invalid)).is_err());
        }
    }
}
