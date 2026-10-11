use super::*;

fn prepare_reasoning(
    client: WireApi,
    upstream: WireApi,
    request: &Value,
    mode: MessagesReasoningMode,
) -> AdapterResult<crate::PreparedAdapterRequest> {
    SourceAdapter::between(client, upstream)
        .unwrap()
        .prepare_request(AdapterRequestContext {
            client_wire_api: client,
            request,
            model: "test",
            stream: false,
            reasoning_mode: mode,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "reasoning-test",
            response_id_seed: "request-test",
        })
}

#[test]
fn reasoning_effort_translation_keeps_target_modes_consistent_across_clients() {
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        for upstream in [WireApi::Messages, WireApi::Gemini] {
            let adapter = SourceAdapter::between(client, upstream).unwrap();
            for (mode, effort, accepted, expected) in [
                (MessagesReasoningMode::Disabled, "high", false, Value::Null),
                (MessagesReasoningMode::Adaptive, "high", true, json!("high")),
                (
                    MessagesReasoningMode::Budget,
                    "very_high",
                    true,
                    json!(24_576),
                ),
            ] {
                let request = if client == WireApi::Responses {
                    json!({"input":"Hello","reasoning":{"effort":effort},"max_output_tokens":32768})
                } else {
                    json!({"messages":[{"role":"user","content":"Hello"}],"reasoning_effort":effort,"max_completion_tokens":32768})
                };
                let prepared = prepare_reasoning(client, upstream, &request, mode);
                assert_eq!(
                    prepared.is_ok(),
                    accepted,
                    "{client:?} -> {upstream:?}, {mode:?}"
                );
                assert_eq!(adapter.supports_reasoning_effort(mode, effort), accepted);
                if let Ok(prepared) = prepared {
                    let body = prepared.upstream_body();
                    let actual = match (upstream, mode) {
                        (WireApi::Messages, MessagesReasoningMode::Budget) => {
                            assert_eq!(body["thinking"]["type"], "enabled");
                            assert!(body.pointer("/output_config/effort").is_none());
                            &body["thinking"]["budget_tokens"]
                        }
                        (WireApi::Messages, _) => {
                            assert_eq!(body["thinking"]["type"], "adaptive");
                            &body["output_config"]["effort"]
                        }
                        (WireApi::Gemini, MessagesReasoningMode::Budget) => {
                            assert!(body
                                .pointer("/generationConfig/thinkingConfig/thinkingLevel")
                                .is_none());
                            &body["generationConfig"]["thinkingConfig"]["thinkingBudget"]
                        }
                        _ => &body["generationConfig"]["thinkingConfig"]["thinkingLevel"],
                    };
                    assert_eq!(actual, &expected);
                }
            }
        }
    }
}

#[test]
fn messages_adaptive_reasoning_carries_very_high_without_downgrading() {
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        let adapter = SourceAdapter::between(client, WireApi::Messages).unwrap();
        for effort in ["xhigh", "extra_high", "Very high"] {
            let request = if client == WireApi::Responses {
                json!({"input":"Hello","reasoning":{"effort":effort}})
            } else {
                json!({"messages":[{"role":"user","content":"Hello"}],"reasoning_effort":effort})
            };
            assert!(adapter.supports_reasoning_effort(MessagesReasoningMode::Adaptive, effort));
            let prepared = prepare_reasoning(
                client,
                WireApi::Messages,
                &request,
                MessagesReasoningMode::Adaptive,
            )
            .unwrap();
            assert_eq!(prepared.upstream_body()["output_config"]["effort"], "xhigh");
            assert_eq!(prepared.upstream_body()["thinking"]["type"], "adaptive");
        }
    }
}

#[test]
fn openai_translation_preserves_explicit_effort_independently_of_messages_mode() {
    let request =
        json!({"messages":[{"role":"user","content":"Hello"}],"reasoning_effort":"vendor_level"});
    assert!(SourceAdapter::ChatCompletionsToResponses
        .supports_reasoning_effort(MessagesReasoningMode::Disabled, "vendor_level"));
    let prepared = prepare_reasoning(
        WireApi::ChatCompletions,
        WireApi::Responses,
        &request,
        MessagesReasoningMode::Disabled,
    )
    .unwrap();
    assert_eq!(
        prepared.upstream_body()["reasoning"]["effort"],
        "vendor_level"
    );
    let aliases =
        json!({"messages":[{"role":"user","content":"Hello"}],"reasoning_effort":"Extra high"});
    let prepared = prepare_reasoning(
        WireApi::ChatCompletions,
        WireApi::Responses,
        &aliases,
        MessagesReasoningMode::Disabled,
    )
    .unwrap();
    assert_eq!(prepared.upstream_body()["reasoning"]["effort"], "xhigh");
}

#[test]
fn malformed_or_unrepresentable_reasoning_fields_are_rejected_before_translation() {
    for (client, fields) in [
        (WireApi::Messages, json!({"output_config":{"effort":42}})),
        (WireApi::Messages, json!({"output_config":{"effort":"  "}})),
        (
            WireApi::Messages,
            json!({"thinking":{"type":"disabled"},"output_config":{"effort":"high"}}),
        ),
        (
            WireApi::Messages,
            json!({"thinking":{"type":"adaptive"},"output_config":{"effort":"none"}}),
        ),
        (
            WireApi::Messages,
            json!({"thinking":{"type":"enabled","budget_tokens":2048},"output_config":{"effort":"high"}}),
        ),
        (
            WireApi::Gemini,
            json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":42}}}),
        ),
        (
            WireApi::Gemini,
            json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":" "}}}),
        ),
        (
            WireApi::Gemini,
            json!({"generationConfig":{"thinkingConfig":{"thinkingLevel":"high","thinkingBudget":2048}}}),
        ),
    ] {
        let mut request = input(client);
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let native =
            prepare_reasoning(client, client, &request, MessagesReasoningMode::Disabled).unwrap();
        for (field, expected) in fields.as_object().unwrap() {
            assert_eq!(&native.upstream_body()[field], expected);
        }
        assert!(prepare_reasoning(
            client,
            WireApi::Responses,
            &request,
            MessagesReasoningMode::Adaptive,
        )
        .is_err());
    }
}

#[test]
fn budget_reasoning_translation_preserves_exact_controls_and_explicit_limits() {
    for (client, upstream, fields, pointer) in [
        (
            WireApi::Messages,
            WireApi::Gemini,
            json!({"thinking":{"type":"enabled","budget_tokens":2048},"max_tokens":4096}),
            "/generationConfig/thinkingConfig/thinkingBudget",
        ),
        (
            WireApi::Gemini,
            WireApi::Messages,
            json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":2048},"maxOutputTokens":4096}}),
            "/thinking/budget_tokens",
        ),
    ] {
        let mut request = input(client);
        request
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let prepared =
            prepare_reasoning(client, upstream, &request, MessagesReasoningMode::Adaptive).unwrap();
        assert_eq!(
            prepared.upstream_body().pointer(pointer),
            Some(&json!(2048))
        );
        let output_limit = if upstream == WireApi::Messages {
            "/max_tokens"
        } else {
            "/generationConfig/maxOutputTokens"
        };
        assert_eq!(
            prepared.upstream_body().pointer(output_limit),
            Some(&json!(4096))
        );
    }
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        let mut request = if client == WireApi::Responses {
            json!({"input":"Hello","reasoning":{"effort":"high"},"max_output_tokens":8192})
        } else {
            json!({"messages":[{"role":"user","content":"Hello"}],"reasoning_effort":"high","max_completion_tokens":8192})
        };
        assert!(prepare_reasoning(
            client,
            WireApi::Messages,
            &request,
            MessagesReasoningMode::Budget
        )
        .is_err());
        request
            .as_object_mut()
            .unwrap()
            .remove(if client == WireApi::Responses {
                "max_output_tokens"
            } else {
                "max_completion_tokens"
            });
        let prepared = prepare_reasoning(
            client,
            WireApi::Messages,
            &request,
            MessagesReasoningMode::Budget,
        )
        .unwrap();
        assert_eq!(
            prepared.upstream_body()["thinking"]["budget_tokens"],
            16_384
        );
        assert!(prepared.upstream_body()["max_tokens"].as_u64().unwrap() > 16_384);
    }
}

#[test]
fn disabled_reasoning_is_preserved_without_enabling_provider_defaults() {
    for client in WireApi::ALL {
        let mut request = input(client);
        request.as_object_mut().unwrap().extend(
            match client {
                WireApi::Responses => json!({"reasoning":{"effort":"none"}}),
                WireApi::ChatCompletions => json!({"reasoning_effort":"none"}),
                WireApi::Messages => json!({"thinking":{"type":"disabled"}}),
                WireApi::Gemini => {
                    json!({"generationConfig":{"thinkingConfig":{"thinkingBudget":0}}})
                }
            }
            .as_object()
            .unwrap()
            .clone(),
        );
        for upstream in WireApi::ALL
            .into_iter()
            .filter(|upstream| *upstream != client)
        {
            for mode in [
                MessagesReasoningMode::Disabled,
                MessagesReasoningMode::Adaptive,
                MessagesReasoningMode::Budget,
            ] {
                let adapter = SourceAdapter::between(client, upstream).unwrap();
                assert!(adapter.supports_reasoning_effort(mode, "none"));
                let prepared = prepare_reasoning(client, upstream, &request, mode).unwrap();
                let body = prepared.upstream_body();
                match upstream {
                    WireApi::Responses => assert_eq!(body["reasoning"]["effort"], "none"),
                    WireApi::ChatCompletions => assert_eq!(body["reasoning_effort"], "none"),
                    WireApi::Messages => assert_eq!(body["thinking"], json!({"type":"disabled"})),
                    WireApi::Gemini => assert_eq!(
                        body["generationConfig"]["thinkingConfig"],
                        json!({"thinkingBudget":0})
                    ),
                }
            }
        }
    }
}

fn chat_request_with_lookup_tool() -> Value {
    json!({
        "model": "test",
        "messages": [{"role": "user", "content": "Hello"}],
        "tools": [{"type": "function", "function": {
            "name": "lookup",
            "parameters": {"type": "object"}
        }}]
    })
}

#[test]
fn chat_responses_preserve_reasoning_and_tool_continuations_from_each_protocol() {
    for (upstream, body) in [
        (
            WireApi::Responses,
            json!({"status":"completed","output":[
            {"type":"reasoning","summary":[{"type":"summary_text","text":"Check first"}]},
            {"type":"function_call","call_id":"call_test","name":"lookup","arguments":"{}"}
        ],"usage":{"input_tokens":3,"output_tokens":2}}),
        ),
        (
            WireApi::Messages,
            json!({"stop_reason":"tool_use","content":[
            {"type":"thinking","thinking":"Check first","signature":"synthetic-signature"},
            {"type":"tool_use","id":"call_test","name":"lookup","input":{}}
        ],"usage":{"input_tokens":3,"output_tokens":2}}),
        ),
        (
            WireApi::Gemini,
            json!({"candidates":[{"content":{"parts":[
            {"text":"Check first","thought":true,"thoughtSignature":"synthetic-signature"},
            {"functionCall":{"id":"call_test","name":"lookup","args":{}}}
        ]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":3,"candidatesTokenCount":2}}),
        ),
    ] {
        let translated = prepare(
            WireApi::ChatCompletions,
            upstream,
            &chat_request_with_lookup_tool(),
            false,
        )
        .translate_response_bytes(&serde_json::to_vec(&body).unwrap())
        .unwrap()
        .unwrap();
        let result = translated.response_body();
        let message = &result["choices"][0]["message"];
        assert_eq!(message["reasoning_content"], "Check first", "{upstream:?}");
        assert_eq!(message["tool_calls"][0]["id"], "call_test");
        assert_eq!(result["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(result["usage"]["prompt_tokens"], 3);
        assert_eq!(result["usage"]["completion_tokens"], 2);
        assert!(result["usage"].get("total_tokens").is_none());

        let history = json!({"messages":[{"role":"user","content":"Hello"},message,
            {"role":"tool","tool_call_id":"call_test","content":"found"}]});
        let prepared = SourceAdapter::between(WireApi::ChatCompletions, upstream)
            .unwrap()
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::ChatCompletions,
                request: &history,
                model: "test",
                stream: false,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: Some(translated.continuation().unwrap().1.clone()),
                response_scope: "source-test",
                response_id_seed: "next",
            })
            .unwrap();
        let native_items = match upstream {
            WireApi::Messages => &prepared.upstream_body()["messages"][1]["content"],
            WireApi::Gemini => &prepared.upstream_body()["contents"][1]["parts"],
            _ => &prepared.upstream_body()["input"],
        };
        if upstream == WireApi::Messages {
            assert_eq!(native_items[0]["signature"], "synthetic-signature");
            assert_eq!(native_items[0]["thinking"], "Check first");
            assert_eq!(native_items[1]["id"], "call_test");
        } else if upstream == WireApi::Gemini {
            assert_eq!(native_items[0]["thoughtSignature"], "synthetic-signature");
            assert_eq!(native_items[0]["text"], "Check first");
            assert_eq!(native_items[1]["functionCall"]["id"], "call_test");
        } else {
            assert_eq!(native_items[1]["type"], "reasoning");
            assert_eq!(native_items[1]["summary"][0]["text"], "Check first");
            assert_eq!(native_items[2]["call_id"], "call_test");
        }
    }
}

#[test]
fn chat_stream_keeps_reasoning_deltas_tool_indices_and_usage() {
    let mut bridge = prepare(
        WireApi::ChatCompletions,
        WireApi::Messages,
        &chat_request_with_lookup_tool(),
        true,
    )
    .into_stream_bridge()
    .unwrap();
    let events = [
        json!({"type":"message_start","message":{"id":"msg_test","usage":{"input_tokens":3}}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"Check "}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"first"}}),
        json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"synthetic-signature"}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_test","name":"lookup","input":{}}}),
        json!({"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{}"}}),
        json!({"type":"content_block_stop","index":1}),
        json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}),
        json!({"type":"message_stop"}),
    ];
    for event in events {
        for fragment in format!("data: {event}\n\n").as_bytes().chunks(7) {
            bridge.push(fragment);
        }
    }
    bridge.finish();
    let completed = bridge
        .completed()
        .expect("portable reasoning stream must complete");
    assert_eq!(
        completed.response_body["choices"][0]["message"]["reasoning_content"],
        "Check first"
    );
    let mut chunks = Vec::new();
    while let Some(chunk) = bridge.pop_output() {
        chunks.extend(chunk);
    }
    let stream = String::from_utf8(chunks).unwrap();
    assert!(stream.ends_with("data: [DONE]\n\n"));
    let values = stream
        .split("\n\n")
        .filter_map(|event| event.strip_prefix("data: "))
        .filter(|event| *event != "[DONE]")
        .map(|event| serde_json::from_str::<Value>(event).unwrap())
        .collect::<Vec<_>>();
    let reasoning = values
        .iter()
        .filter_map(|event| {
            event
                .pointer("/choices/0/delta/reasoning_content")
                .and_then(Value::as_str)
        })
        .collect::<String>();
    assert_eq!(reasoning, "Check first");
    let call = values
        .iter()
        .find_map(|event| event.pointer("/choices/0/delta/tool_calls/0"))
        .unwrap();
    assert_eq!(call["index"], 0);
    assert_eq!(call["id"], "call_test");
    assert_eq!(values.last().unwrap()["usage"]["completion_tokens"], 2);
}

#[test]
fn public_summaries_replay_to_chat_without_accepting_encrypted_state() {
    let mut request = json!({"input":[{"type":"reasoning","id":"rs_test",
        "summary":[{"type":"summary_text","text":"Check first"}]},
        {"role":"user","content":"Continue"}]});
    let prepared = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &request,
        false,
    );
    assert_eq!(
        prepared.upstream_body()["messages"][0]["reasoning_content"],
        "Check first"
    );
    request["input"][0]["encrypted_content"] = "synthetic opaque state".into();
    assert!(decode::request(WireApi::Responses, &request).is_err());
    for role in ["user", "system", "tool"] {
        assert!(decode::request(WireApi::ChatCompletions,
            &json!({"messages":[{"role":role,"content":"text","reasoning_content":"invalid owner"}]})).is_err());
    }
}

#[test]
fn responses_continuation_replays_chat_reasoning_with_the_tool_result() {
    let previous_request = prepare(
        WireApi::Responses,
        WireApi::ChatCompletions,
        &json!({
            "model": "test",
            "input": "Hello",
            "tools": [{"type": "function", "name": "lookup", "parameters": {"type": "object"}}]
        }),
        false,
    );
    let completed = previous_request.translate_response_bytes(&serde_json::to_vec(&json!({
        "id":"chat_test","choices":[{"finish_reason":"tool_calls","message":{
            "role":"assistant","content":null,"reasoning_content":"Check first",
            "tool_calls":[{"id":"call_test","type":"function","function":{"name":"lookup","arguments":"{}"}}]
        }}]
    })).unwrap()).unwrap().unwrap();
    let request = json!({"input":[{"type":"function_call_output","call_id":"call_test","output":"found"}],
        "previous_response_id":completed.response_id()});
    let (_, previous) = completed.continuation().unwrap();
    let continued = SourceAdapter::ResponsesToChatCompletions
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: Some(previous.clone()),
            response_scope: "source-test",
            response_id_seed: "next",
        })
        .unwrap();
    let messages = &continued.upstream_body()["messages"];
    assert_eq!(messages[1]["reasoning_content"], "Check first");
    assert_eq!(messages[1]["tool_calls"][0]["id"], "call_test");
    assert_eq!(messages[2]["tool_call_id"], "call_test");
    assert_eq!(messages[2]["content"], "found");
}
