use super::*;
use crate::{AdapterRequestContext, CacheWriteTtl, SourceAdapter};
use serde_json::json;

mod reasoning;

fn input(protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses => json!({"model":"test","input":"Hello"}),
        WireApi::ChatCompletions => {
            json!({"model":"test","messages":[{"role":"user","content":"Hello"}]})
        }
        WireApi::Messages => {
            json!({"model":"test","max_tokens":64,"messages":[{"role":"user","content":"Hello"}]})
        }
        WireApi::Gemini => json!({"contents":[{"role":"user","parts":[{"text":"Hello"}]}]}),
    }
}

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

fn output(protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses => {
            json!({"id":"response_test","object":"response","model":"test","status":"completed","output":[{"type":"message","id":"msg_test","role":"assistant","content":[{"type":"output_text","text":"Hi","annotations":[]}]}],"usage":{"input_tokens":3,"output_tokens":2}})
        }
        WireApi::ChatCompletions => {
            json!({"id":"chat_test","object":"chat.completion","model":"test","choices":[{"index":0,"message":{"role":"assistant","content":"Hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}})
        }
        WireApi::Messages => {
            json!({"id":"msg_test","type":"message","role":"assistant","model":"test","content":[{"type":"text","text":"Hi"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}})
        }
        WireApi::Gemini => {
            json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":"Hi"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}})
        }
    }
}

fn prepare(
    client: WireApi,
    upstream: WireApi,
    request: &Value,
    stream: bool,
) -> crate::PreparedAdapterRequest {
    let adapter = SourceAdapter::between(client, upstream).unwrap();
    adapter
        .prepare_request(AdapterRequestContext {
            client_wire_api: client,
            request,
            model: "test",
            stream,
            reasoning_mode: if adapter.is_passthrough() {
                MessagesReasoningMode::Disabled
            } else {
                MessagesReasoningMode::Adaptive
            },
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "source-test",
            response_id_seed: "request-test",
        })
        .unwrap_or_else(|error| panic!("{client:?} -> {upstream:?}: {error}"))
}

#[test]
fn all_sixteen_request_and_json_contracts() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL {
            let request = input(client);
            let prepared = prepare(client, upstream, &request, false);
            let body = prepared.upstream_body();
            let key = match upstream {
                WireApi::Responses => "input",
                WireApi::Gemini => "contents",
                _ => "messages",
            };
            assert!(body.get(key).is_some(), "{client:?} -> {upstream:?}");
            let translated = prepared
                .translate_response_bytes(&serde_json::to_vec(&output(upstream)).unwrap())
                .unwrap();
            if client == upstream {
                assert!(translated.is_none());
                continue;
            }
            let translated = translated.unwrap();
            let text = match client {
                WireApi::Responses => translated
                    .response_body()
                    .pointer("/output/0/content/0/text"),
                WireApi::ChatCompletions => translated
                    .response_body()
                    .pointer("/choices/0/message/content"),
                WireApi::Messages => translated.response_body().pointer("/content/0/text"),
                WireApi::Gemini => translated
                    .response_body()
                    .pointer("/candidates/0/content/parts/0/text"),
            };
            assert_eq!(
                text.and_then(Value::as_str),
                Some("Hi"),
                "{client:?} -> {upstream:?}"
            );
        }
    }
}

#[test]
fn image_schema_and_reasoning_survive_all_protocol_pairs() {
    let schema = json!({"type":"object","properties":{"caption":{"type":"string","minLength":2}},"required":["caption"],"additionalProperties":false});
    for client in WireApi::ALL {
        let request = match client {
            WireApi::Responses => {
                json!({"input":[{"role":"user","content":[{"type":"input_image","image_url":"data:image/png;base64,AA=="}]}],"max_output_tokens":128,"text":{"format":{"type":"json_schema","name":"caption","schema":schema}},"reasoning":{"effort":"high"}})
            }
            WireApi::ChatCompletions => {
                json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"data:image/png;base64,AA=="}}]}],"max_completion_tokens":128,"response_format":{"type":"json_schema","json_schema":{"name":"caption","schema":schema}},"reasoning_effort":"high"})
            }
            WireApi::Messages => {
                json!({"messages":[{"role":"user","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}}]}],"max_tokens":128,"output_config":{"format":{"type":"json_schema","schema":schema},"effort":"high"},"thinking":{"type":"adaptive"}})
            }
            WireApi::Gemini => {
                json!({"contents":[{"role":"user","parts":[{"inlineData":{"mimeType":"image/png","data":"AA=="}}]}],"generationConfig":{"maxOutputTokens":128,"responseMimeType":"application/json","responseJsonSchema":schema,"thinkingConfig":{"thinkingLevel":"high"}}})
            }
        };
        for upstream in WireApi::ALL {
            let prepared = prepare(client, upstream, &request, false);
            let (image, format, tokens, effort, expected_image) = match upstream {
                WireApi::Responses => (
                    "/input/0/content/0/image_url",
                    "/text/format/schema",
                    "/max_output_tokens",
                    "/reasoning/effort",
                    "data:image/png;base64,AA==",
                ),
                WireApi::ChatCompletions => (
                    "/messages/0/content/0/image_url/url",
                    "/response_format/json_schema/schema",
                    "/max_completion_tokens",
                    "/reasoning_effort",
                    "data:image/png;base64,AA==",
                ),
                WireApi::Messages => (
                    "/messages/0/content/0/source/data",
                    "/output_config/format/schema",
                    "/max_tokens",
                    "/output_config/effort",
                    "AA==",
                ),
                WireApi::Gemini => (
                    "/contents/0/parts/0/inlineData/data",
                    "/generationConfig/responseJsonSchema",
                    "/generationConfig/maxOutputTokens",
                    "/generationConfig/thinkingConfig/thinkingLevel",
                    "AA==",
                ),
            };
            let body = prepared.upstream_body();
            assert_eq!(
                body.pointer(image),
                Some(&json!(expected_image)),
                "{client:?} -> {upstream:?}"
            );
            assert_eq!(
                body.pointer(format),
                Some(&schema),
                "{client:?} -> {upstream:?}"
            );
            assert_eq!(body.pointer(tokens), Some(&json!(128)));
            assert_eq!(body.pointer(effort), Some(&json!("high")));
        }
    }
}

#[test]
fn claude_cache_write_ttl_is_applied_for_every_client_protocol() {
    for client in WireApi::ALL {
        let request = match client {
            WireApi::Responses => json!({"model":"test","input":"Hello"}),
            WireApi::ChatCompletions => {
                json!({"model":"test","messages":[{"role":"user","content":"Hello"}]})
            }
            WireApi::Messages => {
                json!({"model":"test","max_tokens":64,"messages":[{"role":"user","content":"Hello"}]})
            }
            WireApi::Gemini => {
                json!({"contents":[{"role":"user","parts":[{"text":"Hello"}]}]})
            }
        };
        for (ttl, expected) in [
            (CacheWriteTtl::FiveMinutes, "5m"),
            (CacheWriteTtl::OneHour, "1h"),
        ] {
            let prepared = SourceAdapter::between(client, WireApi::Messages)
                .expect("every client protocol has a Claude bridge")
                .prepare_request(AdapterRequestContext {
                    client_wire_api: client,
                    request: &request,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: ttl,
                    previous: None,
                    response_scope: "source-test",
                    response_id_seed: "cache-test",
                })
                .unwrap();
            let body = prepared.upstream_body();
            let cache_ttl = body
                .pointer("/messages/0/content/0/cache_control/ttl")
                .and_then(Value::as_str);
            assert_eq!(cache_ttl, Some(expected), "{client:?} -> Messages");
        }
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

fn sse(protocol: WireApi) -> Vec<u8> {
    let events = match protocol {
        WireApi::Responses => vec![
            json!({"type":"response.created","response":{"id":"response_test","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_test","role":"assistant","content":[]}}),
            json!({"type":"response.content_part.added","output_index":0,"content_index":0,"part":{"type":"output_text","text":"","annotations":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"content_index":0,"delta":"Hi"}),
            json!({"type":"response.completed","response":output(protocol)}),
        ],
        WireApi::ChatCompletions => vec![
            json!({"id":"chat_test","choices":[{"index":0,"delta":{"role":"assistant","content":"Hi"},"finish_reason":null}]}),
            json!({"id":"chat_test","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
            json!({"id":"chat_test","choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2}}),
        ],
        WireApi::Messages => vec![
            json!({"type":"message_start","message":{"id":"msg_test","type":"message","model":"test","role":"assistant","content":[],"usage":{"input_tokens":3}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}),
            json!({"type":"message_stop"}),
        ],
        WireApi::Gemini => vec![output(protocol)],
    };
    let mut stream = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    if protocol == WireApi::ChatCompletions {
        stream.push_str("data: [DONE]\n\n");
    }
    stream.into_bytes()
}

#[test]
fn all_sixteen_stream_contracts_handle_fragmented_frames_and_completion() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL {
            let request = input(client);
            let prepared = prepare(client, upstream, &request, true);
            let Some(mut bridge) = prepared.into_stream_bridge() else {
                assert_eq!(client, upstream);
                continue;
            };
            for chunk in sse(upstream).chunks(7) {
                bridge.push(chunk);
            }
            bridge.finish();
            assert!(bridge.is_terminal(), "{client:?} -> {upstream:?}");
            assert!(bridge.completed().is_some(), "{client:?} -> {upstream:?}");
            let mut output = Vec::new();
            while let Some(bytes) = bridge.pop_output() {
                output.extend(bytes);
            }
            let output = String::from_utf8(output).unwrap();
            assert!(output.contains("Hi"));
            assert!(
                !output.contains("adapter_upstream_stream_invalid"),
                "{client:?} -> {upstream:?}"
            );
        }
    }
}

#[test]
fn incomplete_stream_never_produces_a_successful_completion() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL {
            if client == upstream {
                continue;
            }
            let mut bridge = prepare(client, upstream, &input(client), true)
                .into_stream_bridge()
                .unwrap();
            bridge.push(b"data: {");
            bridge.finish();
            assert!(bridge.is_terminal());
            assert!(bridge.completed().is_none());
        }
    }
}

fn tool_stream(protocol: WireApi) -> Vec<u8> {
    let mut events = Vec::new();
    match protocol {
        WireApi::Responses => {
            events.push(json!({"type":"response.created","response":{"id":"resp_tools","output":[]}}));
            let mut output = Vec::new();
            for index in 0..2 {
                let item = json!({"type":"function_call","id":format!("fc_{index}"),"call_id":format!("call_new_{index}"),"name":"lookup","arguments":""});
                events.push(json!({"type":"response.output_item.added","output_index":index,"item":item}));
                let mut complete = item;
                complete["arguments"] = format!("{{\"value\":{index}}}").into();
                output.push(complete);
            }
            for fragment in ["{\"value\":", "0}"] {
                for index in 0..2 {
                    events.push(json!({"type":"response.function_call_arguments.delta","output_index":index,"delta":if fragment == "0}" { format!("{index}}}") } else { fragment.into() }}));
                }
            }
            events.push(json!({"type":"response.completed","response":{"id":"resp_tools","status":"completed","output":output,"usage":{"input_tokens":11,"output_tokens":7}}}));
        }
        WireApi::ChatCompletions => {
            for index in 0..2 {
                events.push(json!({"id":"chat_tools","choices":[{"index":0,"delta":{"tool_calls":[{"index":index,"id":format!("call_new_{index}"),"type":"function","function":{"name":"lookup","arguments":"{\"value\":"}}]}}]}));
            }
            for index in 0..2 {
                events.push(json!({"id":"chat_tools","choices":[{"index":0,"delta":{"tool_calls":[{"index":index,"function":{"arguments":format!("{index}}}")}}]}}]}));
            }
            events.push(json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":7}}));
        }
        WireApi::Messages => {
            events.push(json!({"type":"message_start","message":{"id":"msg_tools","usage":{"input_tokens":11}}}));
            for index in 0..2 {
                events.push(json!({"type":"content_block_start","index":index,"content_block":{"type":"tool_use","id":format!("call_new_{index}"),"name":"lookup","input":{}}}));
                for fragment in ["{\"value\":".to_string(), format!("{index}}}")] {
                    events.push(json!({"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":fragment}}));
                }
                events.push(json!({"type":"content_block_stop","index":index}));
            }
            events.push(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}));
            events.push(json!({"type":"message_stop"}));
        }
        WireApi::Gemini => events.push(json!({"candidates":[{"index":0,"content":{"role":"model","parts":[
            {"functionCall":{"id":"call_new_0","name":"lookup","args":{"value":0}}},
            {"functionCall":{"id":"call_new_1","name":"lookup","args":{"value":1}}}
        ]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":11,"candidatesTokenCount":7}})),
    }
    let mut bytes = events
        .iter()
        .map(|value| format!("data: {value}\n\n"))
        .collect::<String>();
    if protocol == WireApi::ChatCompletions {
        bytes.push_str("data: [DONE]\n\n");
    }
    bytes.into_bytes()
}

#[test]
fn every_stream_conversion_keeps_tool_ids_order_arguments_and_actual_usage() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL {
            let Some(mut bridge) =
                prepare(client, upstream, &tool_history(client), true).into_stream_bridge()
            else {
                continue;
            };
            for chunk in tool_stream(upstream).chunks(3) {
                bridge.push(chunk);
            }
            bridge.finish();
            let completed = bridge
                .completed()
                .unwrap_or_else(|| panic!("{client:?} -> {upstream:?}"));
            let result = response::decode(client, &completed.response_body, "test").unwrap();
            let calls = result
                .blocks
                .iter()
                .filter_map(|block| match block {
                    Block::ToolCall {
                        id,
                        name,
                        arguments,
                    } => Some((
                        id.as_str(),
                        name.as_str(),
                        serde_json::from_str::<Value>(arguments).unwrap(),
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>();
            assert_eq!(
                calls,
                [
                    ("call_new_0", "lookup", json!({"value":0})),
                    ("call_new_1", "lookup", json!({"value":1}))
                ],
                "{client:?} -> {upstream:?}"
            );
            assert_eq!(
                (result.usage.input, result.usage.output),
                (Some(11), Some(7))
            );
            let mut chunks = Vec::new();
            while let Some(bytes) = bridge.pop_output() {
                chunks.extend(bytes);
            }
            let events = String::from_utf8(chunks).unwrap();
            assert!(events.contains("call_new_0") && events.contains("call_new_1"));
            assert!(!events.contains("adapter_upstream_stream_invalid"));
        }
    }
}

#[test]
fn unsupported_parameters_fail_before_transport_but_native_extensions_survive() {
    for client in WireApi::ALL {
        let mut value = input(client);
        value["provider_extension"] = json!({"must_preserve":true});
        let native = prepare(client, client, &value, false);
        assert_eq!(
            native.upstream_body()["provider_extension"],
            value["provider_extension"]
        );
        for upstream in WireApi::ALL {
            if client == upstream || client == WireApi::Responses {
                continue;
            }
            let error = TranslationRequest::prepare(
                AdapterRequestContext {
                    client_wire_api: client,
                    request: &value,
                    model: "test",
                    stream: false,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: None,
                    response_scope: "source",
                    response_id_seed: "request",
                },
                upstream,
            )
            .unwrap_err();
            assert_eq!(
                error.code(),
                crate::error_codes::ADAPTER_PARAMETER_UNSUPPORTED
            );
        }
    }
}

#[test]
fn absent_usage_remains_unknown() {
    let usage = response::usage(
        WireApi::ChatCompletions,
        &json!({"usage":{"completion_tokens":7}}),
    );
    for protocol in WireApi::ALL {
        let value = response::usage_value(protocol, &usage);
        assert!(value.get("input_tokens").is_none());
        assert!(value.get("prompt_tokens").is_none());
        assert!(value.get("promptTokenCount").is_none());
        assert!(value.get("total_tokens").is_none());
        assert!(value.get("totalTokenCount").is_none());
    }
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

    let encoded = response::encode(WireApi::Messages, &response, "test").unwrap();
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

fn tool_history(protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses => json!({"model":"test","instructions":"Be brief","input":[
            {"role":"user","content":"Check the test value"},
            {"type":"function_call","call_id":"call_first","name":"lookup","arguments":"{\"value\":1}"},
            {"type":"function_call_output","call_id":"call_first","output":"found"},
            {"type":"function_call","call_id":"call_second","name":"lookup","arguments":"{\"value\":2}"},
            {"type":"function_call_output","call_id":"call_second","output":"missing"}
        ],"tools":[{"type":"function","name":"lookup","parameters":{"type":"object","properties":{"value":{"type":"integer"}}}}],"tool_choice":"auto"}),
        WireApi::ChatCompletions => json!({"model":"test","messages":[
            {"role":"system","content":"Be brief"},{"role":"user","content":"Check the test value"},
            {"role":"assistant","tool_calls":[{"id":"call_first","type":"function","function":{"name":"lookup","arguments":"{\"value\":1}"}}]},
            {"role":"tool","tool_call_id":"call_first","content":"found"},
            {"role":"assistant","tool_calls":[{"id":"call_second","type":"function","function":{"name":"lookup","arguments":"{\"value\":2}"}}]},
            {"role":"tool","tool_call_id":"call_second","content":"missing"}
        ],"tools":[{"type":"function","function":{"name":"lookup","parameters":{"type":"object","properties":{"value":{"type":"integer"}}}}}],"tool_choice":"auto"}),
        WireApi::Messages => json!({"model":"test","max_tokens":64,"system":"Be brief","messages":[
            {"role":"user","content":"Check the test value"},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_first","name":"lookup","input":{"value":1}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_first","content":"found"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"call_second","name":"lookup","input":{"value":2}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"call_second","content":"missing"}]}
        ],"tools":[{"name":"lookup","input_schema":{"type":"object","properties":{"value":{"type":"integer"}}}}],"tool_choice":{"type":"auto"}}),
        WireApi::Gemini => json!({"systemInstruction":{"parts":[{"text":"Be brief"}]},"contents":[
            {"role":"user","parts":[{"text":"Check the test value"}]},
            {"role":"model","parts":[{"functionCall":{"id":"call_first","name":"lookup","args":{"value":1}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"call_first","name":"lookup","response":{"result":"found"}}}]},
            {"role":"model","parts":[{"functionCall":{"id":"call_second","name":"lookup","args":{"value":2}}}]},
            {"role":"user","parts":[{"functionResponse":{"id":"call_second","name":"lookup","response":{"result":"missing"}}}]}
        ],"tools":[{"functionDeclarations":[{"name":"lookup","parametersJsonSchema":{"type":"object","properties":{"value":{"type":"integer"}}}}]}],"toolConfig":{"functionCallingConfig":{"mode":"AUTO"}}}),
    }
}

#[test]
fn all_sixteen_paths_preserve_multi_turn_tool_history_and_instructions() {
    for client in WireApi::ALL {
        for upstream in WireApi::ALL {
            let prepared = prepare(client, upstream, &tool_history(client), false);
            let mut request = decode::request(upstream, prepared.upstream_body())
                .unwrap_or_else(|error| panic!("{client:?} -> {upstream:?}: {error}"));
            decode::resolve_tool_history(&mut request.messages).unwrap();
            let blocks = request
                .messages
                .iter()
                .flat_map(|message| &message.blocks)
                .collect::<Vec<_>>();
            for expected in ["call_first", "call_second"] {
                assert!(blocks.iter().any(|block| matches!(block, Block::ToolCall { id, name, .. } if id == expected && name == "lookup")), "{client:?} -> {upstream:?}");
                assert!(
                    blocks.iter().any(
                        |block| matches!(block, Block::ToolResult { id, .. } if id == expected)
                    ),
                    "{client:?} -> {upstream:?}"
                );
            }
            assert!(
                request.instructions.is_some()
                    || request
                        .messages
                        .iter()
                        .any(|message| message.role == Role::System)
            );
            assert_eq!(request.tools[0].name, "lookup");
        }
    }
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
fn messages_refusal_keeps_its_terminal_reason_across_json_translation() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let prepared = prepare(client, WireApi::Messages, &input(client), false);
        let completed = prepared
            .translate_response_bytes(
                &serde_json::to_vec(&json!({
                    "id":"msg_refusal", "stop_reason":"refusal",
                    "content":[{"type":"text","text":"I cannot help with that."}],
                    "usage":{"input_tokens":10,"output_tokens":7}
                }))
                .unwrap(),
            )
            .unwrap()
            .unwrap();
        let decoded = response::decode(client, completed.response_body(), "seed").unwrap();
        assert_eq!(decoded.finish, Finish::Filter, "{client:?}");
    }
}

#[test]
fn messages_refusal_stream_completes_with_client_refusal_reason() {
    for client in [
        WireApi::Responses,
        WireApi::ChatCompletions,
        WireApi::Gemini,
    ] {
        let mut bridge = prepare(client, WireApi::Messages, &input(client), true)
            .into_stream_bridge()
            .unwrap();
        for event in [
            json!({"type":"message_start","message":{"id":"msg_refusal","usage":{"input_tokens":10}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"I cannot help."}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":7}}),
            json!({"type":"message_stop"}),
        ] {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.finish();
        let completed = bridge.completed().expect("refusal is a terminal response");
        if client == WireApi::Responses {
            assert_eq!(completed.response_body["status"], "incomplete");
            assert_eq!(
                completed.response_body["incomplete_details"]["reason"],
                "content_filter"
            );
        }
        let decoded = response::decode(client, &completed.response_body, "seed").unwrap();
        assert_eq!(
            decoded.finish,
            Finish::Filter,
            "{client:?}: {}",
            completed.response_body
        );
        assert_eq!(decoded.usage.input, Some(10), "{client:?}");
        assert_eq!(decoded.usage.output, Some(7), "{client:?}");
        let mut output = Vec::new();
        while let Some(chunk) = bridge.pop_output() {
            output.extend_from_slice(&chunk);
        }
        assert!(!String::from_utf8(output)
            .unwrap()
            .contains("adapter_upstream_stream_invalid"));
    }
}

#[test]
fn chat_refusal_field_and_content_part_convert_without_a_transport_error() {
    for upstream in [
        json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":null,"refusal":"Cannot help."}}]}),
        json!({"choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":[{"type":"refusal","refusal":"Cannot help."}]}}]}),
    ] {
        for client in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
            let completed = prepare(client, WireApi::ChatCompletions, &input(client), false)
                .translate_response_bytes(&serde_json::to_vec(&upstream).unwrap())
                .unwrap()
                .unwrap();
            let decoded = response::decode(client, completed.response_body(), "seed").unwrap();
            assert_eq!(decoded.finish, Finish::Filter, "{client:?}");
            assert!(matches!(&decoded.blocks[0], Block::Text(text) if text == "Cannot help."));
        }
    }
}

#[test]
fn chat_refusal_stream_preserves_text_and_terminal_reason() {
    for client in [WireApi::Responses, WireApi::Messages, WireApi::Gemini] {
        let mut bridge = prepare(client, WireApi::ChatCompletions, &input(client), true)
            .into_stream_bridge()
            .unwrap();
        for event in [
            json!({"choices":[{"index":0,"delta":{"role":"assistant","refusal":"Cannot "},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"refusal":"help."},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
        ] {
            bridge.push(format!("data: {event}\n\n").as_bytes());
        }
        bridge.push(b"data: [DONE]\n\n");
        let completed = bridge.completed().expect("refusal stream should complete");
        let decoded = response::decode(client, &completed.response_body, "seed").unwrap();
        assert_eq!(decoded.finish, Finish::Filter, "{client:?}");
        assert!(matches!(&decoded.blocks[0], Block::Text(text) if text == "Cannot help."));
    }
}

#[test]
fn gemini_prompt_block_without_candidates_keeps_filtered_terminal_in_json_and_stream() {
    for reason in [
        "SAFETY",
        "OTHER",
        "BLOCKLIST",
        "PROHIBITED_CONTENT",
        "IMAGE_SAFETY",
    ] {
        let blocked = json!({"promptFeedback":{"blockReason":reason},"usageMetadata":{"promptTokenCount":3},"candidates":[]});
        for client in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
        ] {
            let request = input(client);
            let completed = prepare(client, WireApi::Gemini, &request, false)
                .translate_response_bytes(&serde_json::to_vec(&blocked).unwrap())
                .unwrap()
                .expect("converted Gemini response");
            let decoded = response::decode(client, completed.response_body(), "seed").unwrap();
            assert_eq!(decoded.finish, Finish::Filter, "{client:?}: {reason}");
            assert!(decoded
                .blocks
                .iter()
                .all(|block| matches!(block, Block::Text(text) if text.is_empty())));
            assert_eq!(decoded.usage.input, Some(3));

            let mut bridge = prepare(client, WireApi::Gemini, &request, true)
                .into_stream_bridge()
                .unwrap();
            bridge.push(format!("data: {blocked}\n\n").as_bytes());
            let streamed = bridge
                .completed()
                .expect("blocked prompt terminates stream");
            let decoded = response::decode(client, &streamed.response_body, "seed").unwrap();
            assert_eq!(decoded.finish, Finish::Filter, "{client:?}: {reason}");
            assert!(decoded
                .blocks
                .iter()
                .all(|block| matches!(block, Block::Text(text) if text.is_empty())));
            assert_eq!(decoded.usage.input, Some(3));
            if client == WireApi::Responses {
                let frames = std::iter::from_fn(|| bridge.pop_output())
                    .map(|frame| String::from_utf8(frame).unwrap())
                    .collect::<String>();
                assert!(frames.contains("event: response.incomplete"));
                assert!(!frames.contains("event: response.completed"));
            }
        }
    }
    let without_candidates = json!({"promptFeedback":{"blockReason":"SAFETY"}});
    assert_eq!(
        response::decode(WireApi::Gemini, &without_candidates, "seed")
            .unwrap()
            .finish,
        Finish::Filter
    );

    let mut bridge = prepare(
        WireApi::Responses,
        WireApi::Gemini,
        &input(WireApi::Responses),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    bridge.push(b"data: {\"usageMetadata\":{\"promptTokenCount\":7}}\n\n");
    bridge.push(format!("data: {without_candidates}\n\n").as_bytes());
    let completed = bridge.completed().expect("prompt block after usage");
    assert_eq!(completed.response_body["usage"]["input_tokens"], 7);
}

#[test]
fn gemini_missing_candidates_without_known_prompt_block_never_succeeds() {
    for value in [
        json!({"candidates":[],"usageMetadata":{"promptTokenCount":3}}),
        json!({"promptFeedback":{"blockReason":"BLOCK_REASON_UNSPECIFIED"},"candidates":[]}),
        json!({"promptFeedback":{"blockReason":"SAFETY"},"candidates":[{"finishReason":"STOP"}]}),
        json!({"candidates":[{"finishReason":"FINISH_REASON_UNSPECIFIED","content":{"parts":[{"text":"Hi"}]}}]}),
    ] {
        assert!(response::decode(WireApi::Gemini, &value, "seed").is_err());
        let mut bridge = prepare(
            WireApi::Responses,
            WireApi::Gemini,
            &input(WireApi::Responses),
            true,
        )
        .into_stream_bridge()
        .unwrap();
        bridge.push(format!("data: {value}\n\n").as_bytes());
        bridge.finish();
        assert!(bridge.completed().is_none());
    }
}

#[test]
fn gemini_empty_filtered_or_limited_candidate_is_incomplete_in_json_and_stream() {
    for (reason, finish) in [("SAFETY", Finish::Filter), ("MAX_TOKENS", Finish::Length)] {
        let upstream = json!({"candidates":[{"index":0,"finishReason":reason}],"usageMetadata":{"promptTokenCount":3}});
        for client in [
            WireApi::Responses,
            WireApi::ChatCompletions,
            WireApi::Messages,
        ] {
            let request = input(client);
            let result = prepare(client, WireApi::Gemini, &request, false)
                .translate_response_bytes(&serde_json::to_vec(&upstream).unwrap())
                .unwrap()
                .unwrap();
            let decoded = response::decode(client, result.response_body(), "seed").unwrap();
            assert_eq!(decoded.finish, finish, "{client:?}: {reason}");
            assert!(decoded
                .blocks
                .iter()
                .all(|block| matches!(block, Block::Text(text) if text.is_empty())));
            if client == WireApi::Responses {
                assert_eq!(
                    result.response_body()["incomplete_details"]["reason"],
                    if finish == Finish::Length {
                        "max_output_tokens"
                    } else {
                        "content_filter"
                    }
                );
            }

            let mut bridge = prepare(client, WireApi::Gemini, &request, true)
                .into_stream_bridge()
                .unwrap();
            bridge.push(format!("data: {upstream}\n\n").as_bytes());
            let result = bridge
                .completed()
                .expect("incomplete candidate terminates stream");
            let decoded = response::decode(client, &result.response_body, "seed").unwrap();
            assert_eq!(decoded.finish, finish, "{client:?}: {reason}");
            if client == WireApi::Responses {
                let frames = std::iter::from_fn(|| bridge.pop_output())
                    .map(|frame| String::from_utf8(frame).unwrap())
                    .collect::<String>();
                assert!(frames.contains("event: response.incomplete"));
                assert!(!frames.contains("event: response.completed"));
            }
        }
    }
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
