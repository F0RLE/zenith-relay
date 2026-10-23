use super::*;

fn prepare_history(
    upstream: WireApi,
    request: &Value,
    previous: Option<AdapterContinuationState>,
) -> AdapterResult<PreparedAdapterRequest> {
    SourceAdapter::between(WireApi::Responses, upstream)
        .unwrap()
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request,
            model: "test",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous,
            response_scope: "source-test",
            response_id_seed: "request-test",
        })
}

fn history_request(input: Value) -> Value {
    json!({
        "input": input,
        "reasoning": {"effort": "high"},
        "tools": [{"type":"function","name":"read_file","parameters":{"type":"object"}}]
    })
}

fn native_call_response(upstream: WireApi) -> Value {
    match upstream {
        WireApi::Messages => json!({
            "id":"message-test","type":"message","role":"assistant",
            "stop_reason":"tool_use",
            "content":[
                {"type":"thinking","thinking":"Check file","signature":"synthetic-signature"},
                {"type":"text","text":"Reading"},
                {"type":"tool_use","id":"call_test","name":"read_file","input":{}}
            ]
        }),
        WireApi::Gemini => json!({"candidates":[{
            "content":{"role":"model","parts":[
                {"text":"Check file","thought":true},
                {"functionCall":{"id":"call_test","name":"read_file","args":{}},
                    "thoughtSignature":"synthetic-signature"}
            ]},"finishReason":"STOP"
        }]}),
        WireApi::ChatCompletions => json!({
            "id":"chat-test","choices":[{"finish_reason":"tool_calls","message":{
                "role":"assistant","content":"Reading","reasoning_content":"Check file",
                "tool_calls":[{"id":"call_test","type":"function",
                    "function":{"name":"read_file","arguments":"{}"}}]
            }}]
        }),
        WireApi::Responses => unreachable!(),
    }
}

#[test]
fn full_responses_history_restores_native_tool_state_on_each_bridge() {
    for upstream in [WireApi::Messages, WireApi::Gemini, WireApi::ChatCompletions] {
        let first_input = json!([{"role":"user","content":"Read the file"}]);
        let first = prepare_history(upstream, &history_request(first_input.clone()), None).unwrap();
        let response = first
            .translate_response_bytes(&serde_json::to_vec(&native_call_response(upstream)).unwrap())
            .unwrap()
            .unwrap();
        let (_, state) = response.continuation().unwrap();
        let mut store = AdapterContinuationStore::new(8, 1_000);
        store.insert(
            "key",
            response.response_id(),
            "source-test",
            state.clone(),
            1,
        );
        let mut input = first_input.as_array().unwrap().clone();
        input.extend(
            response.response_body()["output"]
                .as_array()
                .unwrap()
                .iter()
                .cloned(),
        );
        let call_id = input
            .iter()
            .find_map(|item| item.get("call_id"))
            .unwrap()
            .clone();
        input.push(json!({"type":"function_call_output","call_id":call_id,"output":"contents"}));
        let request = history_request(Value::Array(input));
        let key = SourceAdapter::between(WireApi::Responses, upstream)
            .unwrap()
            .continuation_key(WireApi::Responses, &request, "test")
            .unwrap()
            .unwrap();
        let restored = store.get("key", &key, "source-test", 2).unwrap();
        let continued = prepare_history(upstream, &request, Some(restored)).unwrap();
        let body = continued.upstream_body();
        match upstream {
            WireApi::Messages => {
                assert_eq!(body["messages"].as_array().unwrap().len(), 3);
                assert_eq!(
                    body["messages"][1]["content"],
                    native_call_response(upstream)["content"]
                );
                assert_eq!(body["messages"][2]["content"][0]["tool_use_id"], call_id);
                assert_eq!(body["thinking"]["type"], "adaptive");
            }
            WireApi::Gemini => {
                assert_eq!(body["contents"].as_array().unwrap().len(), 3);
                assert_eq!(
                    body["contents"][1]["parts"],
                    native_call_response(upstream)["candidates"][0]["content"]["parts"]
                );
                assert_eq!(
                    body["contents"][2]["parts"][0]["functionResponse"]["id"],
                    call_id
                );
            }
            WireApi::ChatCompletions => {
                assert_eq!(body["messages"].as_array().unwrap().len(), 3);
                assert_eq!(body["messages"][1]["reasoning_content"], "Check file");
                assert_eq!(body["messages"][2]["tool_call_id"], call_id);
            }
            WireApi::Responses => unreachable!(),
        }
        assert!(store.get("other-key", &key, "source-test", 2).is_err());
        assert!(store.get("key", &key, "other-source", 2).is_err());
        assert!(store.get("key", &key, "source-test", 1_002).is_err());
    }
}

#[test]
fn streamed_custom_tool_full_history_restores_native_gemini_parts() {
    let first_input = json!([
        {"type":"message","role":"developer","content":[{"type":"input_text","text":"Synthetic instructions"}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"Synthetic workspace"}]},
        {"type":"message","role":"user","content":[{"type":"input_text","text":"Create and edit a marker"}]}
    ]);
    let request = json!({
        "input": first_input,
        "reasoning": {"effort":"medium"},
        "tool_choice":"auto",
        "tools":[{"type":"custom","name":"apply_patch","format":{"type":"text"}}]
    });
    let first = prepare_history(WireApi::Gemini, &request, None).unwrap();
    let mut bridge = first.into_stream_bridge().unwrap();
    let native_parts = json!([
        {"text":"Starting.","thoughtSignature":"synthetic-text-signature"},
        {"functionCall":{"name":"apply_patch","args":{"input":"synthetic patch"},"id":"call_synthetic"},
            "thoughtSignature":"synthetic-call-signature"}
    ]);
    let mut upstream_parts = native_parts.as_array().unwrap().clone();
    upstream_parts.insert(0, json!({"text":""}));
    upstream_parts.push(json!({"text":""}));
    for (index, part) in upstream_parts.iter().enumerate() {
        let mut candidate = json!({"content":{"role":"model","parts":[part]}});
        if index == upstream_parts.len() - 1 {
            candidate["finishReason"] = "STOP".into();
        }
        let frame = format!("data: {}\n\n", json!({"candidates":[candidate]}));
        bridge.push(frame.as_bytes());
    }
    bridge.finish();
    let response = bridge.completed().expect("stream should complete");
    let mut store = AdapterContinuationStore::new(8, 1_000);
    store.insert(
        "key",
        &response.response_id,
        "source-test",
        response.continuation.clone(),
        1,
    );

    let mut replay = first_input.as_array().unwrap().clone();
    for mut item in response.response_body["output"].as_array().unwrap().clone() {
        if let Some(content) = item.get_mut("content").and_then(Value::as_array_mut) {
            for part in content {
                part.as_object_mut().unwrap().remove("annotations");
            }
            item.as_object_mut().unwrap().remove("status");
        }
        replay.push(item);
    }
    replay.push(json!({
        "type":"custom_tool_call_output","id":"synthetic-result",
        "call_id":"call_synthetic","output":"Synthetic patch applied"
    }));
    let mut continued_request = request.clone();
    continued_request["input"] = replay.into();
    let key = SourceAdapter::ResponsesToGemini
        .continuation_key(WireApi::Responses, &continued_request, "test")
        .unwrap()
        .unwrap();
    let state = store.get("key", &key, "source-test", 2).unwrap();
    let continued = prepare_history(WireApi::Gemini, &continued_request, Some(state)).unwrap();
    let contents = continued.upstream_body()["contents"].as_array().unwrap();
    assert_eq!(contents.len(), 3);
    assert_eq!(contents[1]["parts"], native_parts);
    assert_eq!(
        contents[2]["parts"][0]["functionResponse"],
        json!({"name":"apply_patch","id":"call_synthetic",
            "response":{"output":"Synthetic patch applied"}})
    );
}

#[test]
fn new_user_turn_migrates_visible_history_without_forwarding_foreign_ciphertext() {
    let request = history_request(json!([
        {"role":"user","content":"Old task"},
        {"type":"reasoning","encrypted_content":"synthetic-foreign-ciphertext",
            "summary":[{"type":"summary_text","text":"Public summary"}]},
        {"role":"assistant","phase":"final_answer","content":"Old answer"},
        {"role":"user","content":"New task"}
    ]));
    for upstream in [WireApi::Messages, WireApi::Gemini, WireApi::ChatCompletions] {
        let prepared = prepare_history(upstream, &request, None).unwrap();
        let body = prepared.upstream_body().to_string();
        assert!(!body.contains("synthetic-foreign-ciphertext"));
        assert!(body.contains("Public summary"));
        assert!(body.contains("Old answer"));
        assert!(body.contains("New task"));
    }
    let current = history_request(json!([
        {"role":"user","content":"Current task"},
        {"type":"reasoning","encrypted_content":"synthetic-foreign-ciphertext","summary":[]},
        {"type":"function_call","call_id":"call_test","name":"read_file","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_test","output":"contents"}
    ]));
    for upstream in [WireApi::Messages, WireApi::Gemini, WireApi::ChatCompletions] {
        assert!(prepare_history(upstream, &current, None).is_err());
    }
}

#[test]
fn model_switch_does_not_hide_unresolved_historic_tool_calls() {
    let request = history_request(json!([
        {"role":"user","content":"Old task"},
        {"type":"reasoning","encrypted_content":"synthetic-foreign-ciphertext","summary":[]},
        {"type":"function_call","call_id":"call_test","name":"read_file","arguments":"{}"},
        {"role":"user","content":"New task"}
    ]));
    assert!(prepare_history(WireApi::Messages, &request, None).is_err());
}

#[test]
fn conflicting_state_cannot_replace_a_valid_response_or_its_history_alias() {
    let first = prepare_history(WireApi::Messages, &history_request(json!("Read")), None).unwrap();
    let response = first
        .translate_response_bytes(
            &serde_json::to_vec(&native_call_response(WireApi::Messages)).unwrap(),
        )
        .unwrap()
        .unwrap();
    let (_, state) = response.continuation().unwrap();
    let mut store = AdapterContinuationStore::new(8, 1_000);
    assert!(store.insert_if_stored(
        "key",
        response.response_id(),
        "source-test",
        state.clone(),
        1
    ));
    let mut conflict = state.clone();
    conflict.native_turns.get_mut(&1).unwrap()[0]["signature"] =
        "different-synthetic-signature".into();
    assert!(!store.insert_if_stored(
        "key",
        response.response_id(),
        "source-test",
        conflict.clone(),
        2
    ));
    assert_eq!(
        store
            .get("key", response.response_id(), "source-test", 3)
            .unwrap(),
        *state
    );
    assert!(store.insert_if_stored("key", "another-response", "source-test", conflict, 3));
    assert_eq!(
        store
            .get("key", state.history_key().unwrap(), "source-test", 4)
            .unwrap_err(),
        AdapterError::continuation_mismatch()
    );
}
