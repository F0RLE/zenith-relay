use super::*;

const THOUGHT: &str = "Synthetic hidden Gemini-client reasoning.";
const SIGNATURE: &str = "synthetic-native-thought-state";
const CALL: &str = "call_display";

fn reply(protocol: WireApi) -> Value {
    match protocol {
        WireApi::Responses => json!({"id":"resp_display","status":"completed","output":[
            {"type":"reasoning","id":"rs_display","summary":[{"type":"summary_text","text":THOUGHT}],"encrypted_content":SIGNATURE},
            {"type":"function_call","id":"fc_display","call_id":CALL,"name":"lookup","arguments":"{\"value\":1}","status":"completed"}
        ],"usage":{"input_tokens":11,"output_tokens":7}}),
        WireApi::ChatCompletions => {
            json!({"id":"chat_display","choices":[{"index":0,"message":{"role":"assistant","reasoning_content":THOUGHT,"content":null,"tool_calls":[{"id":CALL,"type":"function","function":{"name":"lookup","arguments":"{\"value\":1}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":7}})
        }
        WireApi::Messages => {
            json!({"id":"msg_display","type":"message","role":"assistant","content":[
            {"type":"thinking","thinking":THOUGHT,"signature":SIGNATURE},
            {"type":"tool_use","id":CALL,"name":"lookup","input":{"value":1}}
        ],"stop_reason":"tool_use","usage":{"input_tokens":11,"output_tokens":7}})
        }
        _ => unreachable!(),
    }
}

fn wire(protocol: WireApi, native: &Value) -> Vec<u8> {
    let events = match protocol {
        WireApi::Responses => vec![
            json!({"type":"response.created","response":{"id":"resp_display","output":[]}}),
            json!({"type":"response.output_item.added","output_index":0,"item":{"type":"reasoning","id":"rs_display","summary":[]}}),
            json!({"type":"response.reasoning_summary_part.added","output_index":0,"summary_index":0,"part":{"type":"summary_text","text":""}}),
            json!({"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":THOUGHT}),
            json!({"type":"response.reasoning_summary_text.done","output_index":0,"summary_index":0,"text":THOUGHT}),
            json!({"type":"response.reasoning_summary_part.done","output_index":0,"summary_index":0,"part":native["output"][0]["summary"][0]}),
            json!({"type":"response.output_item.done","output_index":0,"item":native["output"][0]}),
            json!({"type":"response.output_item.added","output_index":1,"item":{"type":"function_call","id":"fc_display","call_id":CALL,"name":"lookup","arguments":"","status":"in_progress"}}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"{\"value\":"}),
            json!({"type":"response.function_call_arguments.delta","output_index":1,"delta":"1}"}),
            json!({"type":"response.function_call_arguments.done","output_index":1,"arguments":"{\"value\":1}"}),
            json!({"type":"response.output_item.done","output_index":1,"item":native["output"][1]}),
            json!({"type":"response.completed","response":native}),
        ],
        WireApi::ChatCompletions => vec![
            json!({"choices":[{"index":0,"delta":{"role":"assistant","reasoning_content":THOUGHT},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":CALL,"type":"function","function":{"name":"lookup","arguments":"{\"value\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"1}"}}]},"finish_reason":null}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":native["usage"]}),
        ],
        WireApi::Messages => vec![
            json!({"type":"message_start","message":{"id":"msg_display","type":"message","role":"assistant","content":[],"usage":{"input_tokens":11}}}),
            json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":THOUGHT}}),
            json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":SIGNATURE}}),
            json!({"type":"content_block_stop","index":0}),
            json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":CALL,"name":"lookup","input":{}}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"value\":"}}),
            json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"1}"}}),
            json!({"type":"content_block_stop","index":1}),
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":7}}),
            json!({"type":"message_stop"}),
        ],
        _ => unreachable!(),
    };
    let mut bytes = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    if protocol == WireApi::ChatCompletions {
        bytes.push_str("data: [DONE]\n\n");
    }
    bytes.into_bytes()
}

fn complete(
    prepared: crate::PreparedAdapterRequest,
    streaming: bool,
    native: &Value,
    bytes: &[u8],
    visible_thought: bool,
) -> TranslatedResponse {
    if !streaming {
        return prepared
            .translate_response_bytes(&serde_json::to_vec(native).unwrap())
            .unwrap()
            .unwrap()
            .translated_response()
            .clone();
    }
    let mut stream = prepared.into_stream_bridge().unwrap();
    for fragment in bytes.chunks(3) {
        stream.push(fragment);
    }
    stream.finish();
    let frames = String::from_utf8(
        std::iter::from_fn(|| stream.pop_output())
            .flatten()
            .collect(),
    )
    .unwrap();
    assert_eq!(frames.contains(THOUGHT), visible_thought);
    assert!(!frames.contains(SIGNATURE));
    assert!(stream.completed().is_some(), "{frames}");
    stream.completed().unwrap().clone()
}

#[test]
fn gemini_thought_display_keeps_tool_history_and_native_state_in_json_and_sse() {
    for (upstream, include) in [
        (WireApi::Responses, false),
        (WireApi::Responses, true),
        (WireApi::ChatCompletions, false),
        (WireApi::Messages, false),
        (WireApi::Messages, true),
    ] {
        for streaming in [false, true] {
            let adapter = SourceAdapter::between(WireApi::Gemini, upstream).unwrap();
            let mut request = request(json!(include));
            request["generationConfig"]["thinkingConfig"]["thinkingLevel"] = json!("high");
            request["tools"] = tool_history(WireApi::Gemini)["tools"].clone();
            let native = reply(upstream);
            let prepared = prepare(WireApi::Gemini, upstream, &request, streaming);
            let completed = complete(
                prepared,
                streaming,
                &native,
                &wire(upstream, &native),
                include,
            );
            let content = &completed.response_body["candidates"][0]["content"];
            let mut expected_parts = Vec::new();
            if include {
                expected_parts.push(json!({"text":THOUGHT,"thought":true}));
            }
            expected_parts
                .push(json!({"functionCall":{"id":CALL,"name":"lookup","args":{"value":1}}}));
            assert_eq!(content["parts"], Value::Array(expected_parts));
            assert!(!content.to_string().contains(SIGNATURE));
            assert_eq!(
                completed.response_body["usageMetadata"]["promptTokenCount"],
                11
            );
            assert_eq!(
                completed.response_body["usageMetadata"]["candidatesTokenCount"],
                7
            );
            assert!(
                completed
                    .continuation
                    .portable_history
                    .as_ref()
                    .unwrap()
                    .last()
                    .unwrap()
                    .blocks
                    .iter()
                    .any(|block| matches!(block, Block::Reasoning(text) if text == THOUGHT))
            );
            request["contents"].as_array_mut().unwrap().extend([
                content.clone(),
                json!({"role":"user","parts":[{"functionResponse":{"id":CALL,"name":"lookup","response":{"output":"found"}}}]}),
            ]);
            let next = adapter
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Gemini,
                    request: &request,
                    model: "test",
                    stream: streaming,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: Some(completed.continuation),
                    response_scope: "source-test",
                    response_id_seed: "next",
                })
                .unwrap();
            match upstream {
                WireApi::Responses => {
                    let history = next.upstream_body()["input"].as_array().unwrap();
                    assert_eq!(&history[1..3], native["output"].as_array().unwrap());
                    assert_eq!(history[3]["type"], "function_call_output");
                    assert_eq!(history[3]["call_id"], CALL);
                    assert_eq!(
                        serde_json::from_str::<Value>(history[3]["output"].as_str().unwrap())
                            .unwrap(),
                        json!({"output":"found"})
                    );
                }
                WireApi::ChatCompletions => {
                    let history = next.upstream_body()["messages"].as_array().unwrap();
                    assert_eq!(history[1]["reasoning_content"], THOUGHT);
                    assert_eq!(history[1]["tool_calls"][0]["id"], CALL);
                    assert_eq!(history[2]["role"], "tool");
                    assert_eq!(history[2]["tool_call_id"], CALL);
                    assert_eq!(
                        serde_json::from_str::<Value>(history[2]["content"].as_str().unwrap())
                            .unwrap(),
                        json!({"output":"found"})
                    );
                }
                WireApi::Messages => {
                    let history = next.upstream_body()["messages"].as_array().unwrap();
                    assert_eq!(history[1]["content"], native["content"]);
                    assert_eq!(history[2]["content"][0]["tool_use_id"], CALL);
                    let result = &history[2]["content"][0]["content"];
                    let text = result
                        .as_str()
                        .or_else(|| result.pointer("/0/text").and_then(Value::as_str))
                        .unwrap();
                    assert_eq!(
                        serde_json::from_str::<Value>(text).unwrap(),
                        json!({"output":"found"})
                    );
                }
                _ => unreachable!(),
            }
            let answer = complete(next, streaming, &output(upstream), &sse(upstream), false);
            assert_eq!(
                answer.response_body["candidates"][0]["content"]["parts"],
                json!([{"text":"Hi"}])
            );
        }
    }
}
