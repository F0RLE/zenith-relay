use super::*;
use serde_json::json;

#[test]
fn native_responses_replay_materializes_tool_turn_without_protocol_conversion() {
    let initial = json!({
        "model": "alias",
        "input": "inspect the workspace",
        "tools": [{
            "type": "function",
            "name": "run_command",
            "parameters": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }
        }]
    });
    let upstream = json!({
        "id": "resp_tool_01",
        "output": [{
            "type": "function_call",
            "call_id": "call_01",
            "name": "run_command",
            "arguments": "{\"command\":\"pwd\"}"
        }]
    });
    let (response_id, replay) =
        NativeResponsesReplayState::from_response(&initial, "gpt-test", &upstream)
            .expect("a completed native response is replayable");

    assert_eq!(response_id, "resp_tool_01");
    let continuation = json!({
        "model": "alias",
        "previous_response_id": response_id,
        "input": [{
            "type": "function_call_output",
            "call_id": "call_01",
            "output": "C:\\workspace"
        }],
        "max_output_tokens": 128
    });
    let replayed = replay
        .replay_request(&continuation, "gpt-test", false)
        .expect("the tool result can be replayed as native Responses input");

    assert_eq!(replayed["model"], "gpt-test");
    assert_eq!(replayed["stream"], false);
    assert!(replayed.get("previous_response_id").is_none());
    assert_eq!(replayed["max_output_tokens"], 128);
    assert!(replayed.get("tools").is_none());
    let input = replayed["input"]
        .as_array()
        .expect("replayed input is an array");
    assert_eq!(input.len(), 3);
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[0]["content"][0]["text"], "inspect the workspace");
    assert_eq!(input[1], upstream["output"][0]);
    assert_eq!(input[2], continuation["input"][0]);
}
#[test]
fn native_responses_replay_preserves_current_turn_options() {
    let initial = json!({
        "model": "gpt-test",
        "input": "first",
        "text": {"format": {"type": "text"}},
        "prompt_cache_key": "old-cache",
        "metadata": {"turn": "first"},
        "instructions": "Use the first-turn format",
        "type": "response.create",
        "stream_id": "old-lane",
        "generate": false,
        "max_output_tokens": 1
    });
    let (_, replay) = NativeResponsesReplayState::from_response(
        &initial,
        "gpt-test",
        &json!({"id": "resp_first", "output": []}),
    )
    .unwrap();
    let continuation = json!({
        "model": "alias",
        "previous_response_id": "resp_first",
        "input": "second",
        "text": {"format": {"type": "json_object"}},
        "context_management": [{"type": "compaction", "compact_threshold": 1000}],
        "prompt_cache_key": "new-cache",
        "prompt_cache_retention": "24h",
        "metadata": null,
        "provider_extension": {"value": true}
    });
    let replayed = replay
        .replay_request(&continuation, "gpt-test", true)
        .unwrap();
    for (key, value) in continuation.as_object().unwrap() {
        if !matches!(key.as_str(), "model" | "input" | "previous_response_id") {
            assert_eq!(replayed.get(key), Some(value), "lost current option: {key}");
        }
    }
    assert_eq!(replayed["model"], "gpt-test");
    assert_eq!(replayed["stream"], true);
    assert!(replayed.get("previous_response_id").is_none());
    for key in [
        "instructions",
        "type",
        "stream_id",
        "generate",
        "max_output_tokens",
    ] {
        assert!(replayed.get(key).is_none(), "inherited old option: {key}");
    }
    assert_eq!(replayed["input"].as_array().unwrap().len(), 2);
}
#[test]
fn call_prefixed_function_item_id_repair_keeps_the_tool_result_link() {
    let mut request = json!({
        "input": [
            {"type": "message", "id": "call_message"},
            {
                "type": "function_call",
                "id": "call_function_01",
                "call_id": "call_function_01",
                "name": "run_command",
                "arguments": "{\"command\":\"pwd\"}"
            },
            {
                "type": "function_call",
                "id": "fc_function_02",
                "call_id": "call_function_02",
                "name": "read_file",
                "arguments": "{\"path\":\"Cargo.toml\"}"
            },
            {
                "type": "custom_tool_call",
                "id": "call_custom_01",
                "call_id": "call_custom_01",
                "name": "PowerShell",
                "input": "Get-ChildItem"
            },
            {
                "type": "function_call_output",
                "call_id": "call_function_01",
                "output": "C:\\workspace"
            }
        ]
    });

    assert!(repair_call_prefixed_function_item_ids(&mut request));
    let input = request["input"].as_array().expect("input is an array");
    assert_eq!(input[0]["id"], "call_message");
    assert_eq!(input[1]["id"], "fc_call_function_01");
    assert_eq!(input[1]["call_id"], "call_function_01");
    assert_eq!(input[2]["id"], "fc_function_02");
    assert_eq!(input[3]["id"], "call_custom_01");
    assert_eq!(input[4]["call_id"], "call_function_01");
    assert!(!repair_call_prefixed_function_item_ids(&mut request));
}
#[test]
fn arbitrary_function_item_id_repair_adds_fc_namespace() {
    let mut request = json!({
        "input": [
            {"type": "function_call", "id": "tool_bdr_01", "call_id": "tool_bdr_01"},
            {"type": "function_call", "id": "fc_existing", "call_id": "fc_existing"}
        ]
    });

    assert!(repair_call_prefixed_function_item_ids(&mut request));
    assert_eq!(request["input"][0]["id"], "fc_tool_bdr_01");
    assert_eq!(request["input"][0]["call_id"], "tool_bdr_01");
    assert_eq!(request["input"][1]["id"], "fc_existing");
    assert!(!repair_call_prefixed_function_item_ids(&mut request));
}
#[test]
fn custom_tool_item_id_repair_keeps_the_tool_result_link() {
    let mut request = json!({
        "input": [{
            "type": "custom_tool_call",
            "id": "fc_custom_01",
            "call_id": "toolu_custom_01",
            "name": "PowerShell",
            "input": "Get-ChildItem"
        }, {
            "type": "custom_tool_call_output",
            "call_id": "toolu_custom_01",
            "output": "Cargo.toml"
        }]
    });

    assert!(repair_custom_tool_item_ids(&mut request));
    let input = request["input"].as_array().expect("input is an array");
    assert_eq!(input[0]["id"], "ctc_fc_custom_01");
    assert_eq!(input[0]["call_id"], "toolu_custom_01");
    assert_eq!(input[1]["call_id"], "toolu_custom_01");
    assert!(!repair_custom_tool_item_ids(&mut request));
}
#[test]
fn item_prefixed_message_id_repair_preserves_native_and_tool_item_ids() {
    let mut request = json!({
        "input": [
            {
                "type": "message",
                "id": "item_user_01",
                "role": "user",
                "content": [{"type": "input_text", "text": "Inspect the workspace"}]
            },
            {
                "id": "item_assistant_01",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "I will inspect it."}]
            },
            {
                "type": "message",
                "id": "msg_native_01",
                "role": "developer",
                "content": [{"type": "input_text", "text": "Keep changes scoped."}]
            },
            {
                "type": "function_call",
                "id": "item_function_01",
                "call_id": "call_function_01",
                "name": "run_command",
                "arguments": "{\"command\":\"pwd\"}"
            },
            {
                "type": "reasoning",
                "id": "item_reasoning_01",
                "encrypted_content": "signed-reasoning"
            }
        ]
    });

    assert!(remove_item_prefixed_message_ids(&mut request));
    let input = request["input"].as_array().expect("input is an array");
    assert!(input[0].get("id").is_none());
    assert!(input[1].get("id").is_none());
    assert_eq!(input[2]["id"], "msg_native_01");
    assert_eq!(input[3]["id"], "item_function_01");
    assert_eq!(input[3]["call_id"], "call_function_01");
    assert_eq!(input[4]["id"], "item_reasoning_01");
    assert!(!remove_item_prefixed_message_ids(&mut request));
}
#[test]
fn native_responses_replay_rejects_model_mismatch() {
    let initial = json!({"model": "alias", "input": "inspect"});
    let upstream = json!({"id": "resp_model_01", "output": []});
    let (_, replay) = NativeResponsesReplayState::from_response(&initial, "gpt-test", &upstream)
        .expect("a completed native response is replayable");
    let error = replay
        .replay_request(
            &json!({
                "previous_response_id": "resp_model_01",
                "input": "continue"
            }),
            "other-model",
            false,
        )
        .expect_err("a response cannot cross model routes");

    assert_eq!(error.code(), "adapter_continuation_mismatch");
}
#[test]
fn native_responses_replay_store_is_route_scoped_bounded_and_expiring() {
    let initial = json!({"model": "alias", "input": "inspect"});
    let upstream = json!({"id": "resp_store_01", "output": []});
    let (_, first) = NativeResponsesReplayState::from_response(&initial, "gpt-test", &upstream)
        .expect("a completed native response is replayable");
    let (_, second) = NativeResponsesReplayState::from_response(
        &initial,
        "gpt-test",
        &json!({"id": "resp_store_02", "output": []}),
    )
    .expect("a completed native response is replayable");
    let mut store = NativeResponsesReplayStore::new(1, 10);

    store.insert("key-a", "resp_store_01", "route-a", first, 100);
    assert!(store
        .get("key-a", "resp_store_01", "route-b", 100)
        .is_none());
    assert!(store
        .get("key-b", "resp_store_01", "route-a", 100)
        .is_none());
    assert!(store
        .get("key-a", "resp_store_01", "route-a", 100)
        .is_some());

    store.insert("key-a", "resp_store_02", "route-a", second, 101);
    assert!(store
        .get("key-a", "resp_store_01", "route-a", 101)
        .is_none());
    assert!(store
        .get("key-a", "resp_store_02", "route-a", 112)
        .is_none());
}
#[test]
fn native_replay_does_not_store_an_unresolved_previous_response_reference() {
    let request = json!({
        "model": "alias",
        "input": "continue",
        "previous_response_id": "resp_missing"
    });
    let upstream = json!({"id": "resp_next", "output": []});
    assert!(NativeResponsesReplayState::from_response(&request, "gpt-test", &upstream).is_none());
}
#[test]
fn native_replay_rejects_provider_managed_history() {
    let upstream = json!({"id": "resp_next", "output": []});
    let (_, replay) = NativeResponsesReplayState::from_response(
        &json!({"input": "first"}),
        "gpt-test",
        &upstream,
    )
    .unwrap();
    for request in [
        json!({"input": "continue", "conversation": "conv_external"}),
        json!({"input": "continue", "conversation": {"id": "conv_external"}}),
        json!({"input": [{"type": "item_reference", "id": "msg_external"}]}),
        json!({"input": [{"id": "msg_external"}]}),
    ] {
        assert!(
            NativeResponsesReplayState::from_response(&request, "gpt-test", &upstream).is_none(),
            "provider-managed history cannot become a portable replay"
        );
        assert!(replay.replay_request(&request, "gpt-test", false).is_err());
    }
}
#[test]
fn native_replay_requires_the_matching_tool_call_for_every_output() {
    let call =
        json!({"type":"function_call", "call_id":"call_1", "name":"lookup", "arguments":"{}"});
    let output = json!({"type":"function_call_output", "call_id":"call_1", "output":"value"});
    let completed = json!({"id":"resp_complete", "output":[]});
    for input in [
        json!([output]),
        json!([output, call]),
        json!([call, {"type":"custom_tool_call_output", "call_id":"call_1", "output":"value"}]),
        json!([call, output, output]),
    ] {
        assert!(NativeResponsesReplayState::from_response(
            &json!({"input":input}),
            "model",
            &completed
        )
        .is_none());
    }
    assert!(NativeResponsesReplayState::from_response(
        &json!({"input":[call, output]}),
        "model",
        &completed
    )
    .is_some());
    let (_, replay) = NativeResponsesReplayState::from_response(
        &json!({"input":"start"}),
        "model",
        &json!({"id":"resp_call", "output":[call]}),
    )
    .unwrap();
    assert!(replay
        .replay_request(&json!({"input":[output]}), "model", false)
        .is_ok());
    assert!(replay.replay_request(&json!({"input":[output, {"type":"function_call_output", "call_id":"missing", "output":"value"}]}), "model", false).is_err());
}
#[test]
fn continuation_stores_bound_entry_and_total_retained_bytes() {
    let mut oversized_state =
        MessagesBridgeState::new("claude-test", MessagesReasoningMode::Disabled);
    oversized_state.messages.push(json!({
        "role": "user",
        "content": [{"type": "text", "text": "x".repeat(1_024)}],
    }));
    let mut bridge_store = MessagesBridgeStore::with_limits(4, 60_000, 256, 1_024);
    bridge_store.insert("key-a", "resp_oversized", "route-a", oversized_state, 100);
    assert_eq!(
        bridge_store
            .get("key-a", "resp_oversized", "route-a", 100)
            .expect_err("an oversized continuation is not retained")
            .code(),
        "adapter_continuation_missing"
    );

    let state = |text: &str| {
        let mut state = MessagesBridgeState::new("claude-test", MessagesReasoningMode::Disabled);
        state.messages.push(json!({
            "role": "user",
            "content": [{"type": "text", "text": text}],
        }));
        state
    };
    let mut total_bound_store = MessagesBridgeStore::with_limits(4, 60_000, 2_048, 1_000);
    total_bound_store.insert("key-a", "resp_old", "route-a", state(&"a".repeat(512)), 100);
    total_bound_store.insert("key-a", "resp_new", "route-a", state(&"b".repeat(512)), 101);
    assert_eq!(
        total_bound_store
            .get("key-a", "resp_old", "route-a", 101)
            .expect_err("the oldest state is evicted at the total byte limit")
            .code(),
        "adapter_continuation_missing"
    );
    assert!(total_bound_store
        .get("key-a", "resp_new", "route-a", 101)
        .is_ok());

    let initial = json!({"model": "alias", "input": "x".repeat(1_024)});
    let (_, replay) = NativeResponsesReplayState::from_response(
        &initial,
        "gpt-test",
        &json!({"id": "resp_large_replay", "output": []}),
    )
    .expect("a completed native response is replayable");
    let mut replay_store = NativeResponsesReplayStore::with_limits(4, 60_000, 256, 1_024);
    replay_store.insert("key-a", "resp_large_replay", "route-a", replay, 100);
    assert!(replay_store
        .get("key-a", "resp_large_replay", "route-a", 100)
        .is_none());
}
