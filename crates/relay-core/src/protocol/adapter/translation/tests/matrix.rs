use super::*;
use serde_json::json;

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
