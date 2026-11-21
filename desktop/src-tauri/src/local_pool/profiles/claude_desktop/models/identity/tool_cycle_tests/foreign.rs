use super::*;

const THOUGHT: &str = "Synthetic private fixture reasoning.";
const SIGNATURE: &str = "synthetic-fixture-thought-signature";

pub(super) fn chat_events(model: &Value, tool: bool) -> WireEvents {
    let chunk = |delta: Value, finish: Option<&str>| {
        json!({"id":"chat_fixture_cycle","object":"chat.completion.chunk","created":0,
            "model":model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]})
    };
    let mut events = vec![("", chunk(json!({"role":"assistant"}), None))];
    if tool {
        events.push(("", chunk(json!({"reasoning_content":THOUGHT}), None)));
        events.push(("", chunk(json!({"tool_calls":[{"index":0,"id":CALL,"type":"function","function":{"name":TOOL,"arguments":""}}]}), None)));
        for fragment in ["{\"value\":", "\"synthetic\"}"] {
            events.push((
                "",
                chunk(
                    json!({"tool_calls":[{"index":0,"function":{"arguments":fragment}}]}),
                    None,
                ),
            ));
        }
    } else {
        events.push(("", chunk(json!({"content":ACK}), None)));
    }
    let mut terminal = chunk(json!({}), Some(if tool { "tool_calls" } else { "stop" }));
    terminal["usage"] = json!({"prompt_tokens":5,"completion_tokens":5,"total_tokens":10});
    events.push(("", terminal));
    events
}

pub(super) fn gemini_events(tool: bool) -> WireEvents {
    let mut events = Vec::new();
    if tool {
        events.push(("", json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":THOUGHT,"thought":true,"thoughtSignature":SIGNATURE}]}}]})));
        events.push(("", json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"functionCall":{"id":CALL,"name":TOOL,"args":{"value":"synthetic"}},"thoughtSignature":SIGNATURE}]}}]})));
    } else {
        events.push((
            "",
            json!({"candidates":[{"index":0,"content":{"role":"model","parts":[{"text":ACK}]}}]}),
        ));
    }
    events.push(("", json!({"candidates":[{"index":0,"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":5,"totalTokenCount":10}})));
    events
}

pub(super) fn assert_tool_result(body: &Value, protocol: WireApi) {
    match protocol {
        WireApi::ChatCompletions => {
            let messages = body["messages"].as_array().unwrap();
            let call_turn = messages
                .iter()
                .position(|message| {
                    message["role"] == "assistant" && message["tool_calls"].is_array()
                })
                .unwrap();
            let assistant = &messages[call_turn];
            let calls = assistant["tool_calls"].as_array().unwrap();
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0]["id"], CALL);
            assert_eq!(calls[0]["function"]["name"], TOOL);
            assert_eq!(
                serde_json::from_str::<Value>(calls[0]["function"]["arguments"].as_str().unwrap())
                    .unwrap(),
                json!({"value":"synthetic"})
            );
            assert_eq!(assistant["reasoning_content"], THOUGHT);
            let results = messages
                .iter()
                .filter(|message| message["role"] == "tool")
                .collect::<Vec<_>>();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0]["tool_call_id"], CALL);
            assert_eq!(results[0]["content"], RESULT);
            let result_turn = messages
                .iter()
                .position(|message| message["role"] == "tool")
                .unwrap();
            assert!(call_turn < result_turn);
        }
        WireApi::Gemini => {
            let contents = body["contents"].as_array().unwrap();
            let parts = contents
                .iter()
                .flat_map(|content| content["parts"].as_array().unwrap())
                .collect::<Vec<_>>();
            let call = parts
                .iter()
                .find(|part| part["functionCall"].is_object())
                .unwrap();
            assert_eq!(
                call["functionCall"],
                json!({"id":CALL,"name":TOOL,"args":{"value":"synthetic"}})
            );
            assert_eq!(call["thoughtSignature"], SIGNATURE);
            let thought = parts.iter().find(|part| part["thought"] == true).unwrap();
            assert_eq!(thought["text"], THOUGHT);
            assert_eq!(thought["thoughtSignature"], SIGNATURE);
            let results = parts
                .iter()
                .filter_map(|part| part.get("functionResponse"))
                .collect::<Vec<_>>();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0]["id"], CALL);
            assert_eq!(results[0]["name"], TOOL);
            assert_eq!(results[0]["response"], json!({"output":RESULT}));
            let call_turn = contents
                .iter()
                .position(|content| {
                    content["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|part| part["functionCall"].is_object())
                })
                .unwrap();
            let result_turn = contents
                .iter()
                .position(|content| {
                    content["parts"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|part| part["functionResponse"].is_object())
                })
                .unwrap();
            assert_eq!(contents[call_turn]["role"], "model");
            assert_eq!(contents[result_turn]["role"], "user");
            assert!(call_turn < result_turn);
        }
        _ => unreachable!("foreign fixture only owns Chat and Gemini"),
    }
}

#[test]
fn foreign_tool_cycle_fixtures_preserve_native_reasoning_and_results() {
    use zenith_relay_core::{
        AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter,
    };

    for protocol in [WireApi::ChatCompletions, WireApi::Gemini] {
        let adapter = SourceAdapter::between(WireApi::Messages, protocol).unwrap();
        let mut request = json!({
            "model":"gateway-model","max_tokens":4096,
            "thinking":{"type":"adaptive","display":"omitted"},
            "output_config":{"effort":"high"},
            "tools":[{"name":TOOL,"input_schema":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}}],
            "messages":[{"role":"user","content":"Synthetic tool request."}]
        });
        let mut previous = None;
        for generation in 0..2 {
            let prepared = adapter
                .prepare_request(AdapterRequestContext {
                    client_wire_api: WireApi::Messages,
                    request: &request,
                    model: "gateway-model",
                    stream: true,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: CacheWriteTtl::Provider,
                    previous: previous.take(),
                    response_scope: "synthetic-foreign-fixture",
                    response_id_seed: if generation == 0 { "tool" } else { "answer" },
                })
                .unwrap();
            if generation == 1 {
                assert_tool_result(prepared.upstream_body(), protocol);
            }
            let events = if protocol == WireApi::ChatCompletions {
                chat_events(&json!("gateway-model"), generation == 0)
            } else {
                gemini_events(generation == 0)
            };
            let mut stream = prepared.into_stream_bridge().unwrap();
            for (_, event) in events {
                for fragment in format!("data: {event}\n\n").as_bytes().chunks(3) {
                    stream.push(fragment);
                }
            }
            if protocol == WireApi::ChatCompletions {
                stream.push(b"data: [DONE]\n\n");
            }
            stream.finish();
            let frames = String::from_utf8(
                std::iter::from_fn(|| stream.pop_output())
                    .flatten()
                    .collect(),
            )
            .unwrap();
            assert!(
                stream.completed().is_some(),
                "invalid {protocol:?} fixture generation {generation}: {frames}"
            );
            let completed = stream.completed().unwrap();
            assert_eq!(completed.response_body["usage"]["input_tokens"], 5);
            assert_eq!(completed.response_body["usage"]["output_tokens"], 5);
            let content = &completed.response_body["content"];
            if generation == 0 {
                assert_eq!(
                    content,
                    &json!([{"type":"tool_use","id":CALL,"name":TOOL,"input":{"value":"synthetic"}}])
                );
                request["messages"].as_array_mut().unwrap().extend([
                    json!({"role":"assistant","content":content}),
                    json!({"role":"user","content":[{"type":"tool_result","tool_use_id":CALL,"content":RESULT}]}),
                ]);
                previous = Some(completed.continuation.clone());
            } else {
                assert_eq!(content, &json!([{"type":"text","text":ACK}]));
                assert_eq!(completed.response_body["stop_reason"], "end_turn");
            }
        }
    }
}
