use super::*;
use crate::sources::WireApi;
use serde_json::{json, Value};

fn request(input: Value) -> Value {
    json!({
        "model": "claude-test",
        "input": input,
        "tools": [{
            "type": "function",
            "name": "run_command",
            "description": "Run a command",
            "parameters": {
                "type": "object",
                "properties": {"command": {"type": "string"}},
                "required": ["command"]
            }
        }]
    })
}

#[test]
fn native_prepared_request_is_transparent_for_opaque_tools() {
    let request = json!({
        "model": "alias",
        "input": "inspect",
        "tools": [{
            "type": "computer_use_preview",
            "name": "PowerShell",
            "display_width": 1200,
            "display_height": 800
        }]
    });
    let prepared = SourceAdapter::Native
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Responses,
            request: &request,
            model: "resolved-model",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Disabled,
            previous: None,
            response_scope: "native-route",
        })
        .unwrap();

    assert!(prepared.is_passthrough());
    assert_eq!(prepared.upstream_body()["model"], "resolved-model");
    assert_eq!(prepared.upstream_body()["tools"], request["tools"]);
    assert!(prepared
        .translate_response_bytes(br#"{}"#)
        .unwrap()
        .is_none());
}

#[test]
fn messages_bridge_converts_function_tools_and_preserves_tool_turn_state() {
    let first = prepare_responses_to_messages(
        &request(Value::String("inspect the project".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Adaptive,
        None,
    )
    .unwrap();
    assert_eq!(
        first.upstream_body()["tools"][0]["input_schema"]["type"],
        "object"
    );

    let response = translate_messages_response(
        first,
        &json!({
            "id": "msg_01",
            "model": "claude-test",
            "content": [{
                "type": "tool_use",
                "id": "toolu_01",
                "name": "run_command",
                "input": {"command": "pwd"}
            }],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 12, "output_tokens": 3}
        }),
    )
    .unwrap();
    assert_eq!(response.response_body["output"][0]["type"], "function_call");
    assert_eq!(response.response_body["output"][0]["call_id"], "toolu_01");

    let second = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "previous_response_id": response.response_id,
            "input": [{
                "type": "function_call_output",
                "call_id": "toolu_01",
                "output": "/workspace"
            }]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Adaptive,
        Some(response.continuation),
    )
    .unwrap();
    assert_eq!(
        second.upstream_body()["messages"][2]["content"][0]["type"],
        "tool_result"
    );
    assert_eq!(
        second.upstream_body()["messages"][2]["content"][0]["tool_use_id"],
        "toolu_01"
    );
}

#[test]
fn messages_bridge_preserves_custom_tool_call_and_output_shapes() {
    let first = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "input": "List the project files.",
            "tools": [{
                "type": "custom",
                "name": "PowerShell",
                "description": "Runs one PowerShell command.",
                "format": {
                    "type": "grammar",
                    "syntax": "regex",
                    "definition": "[^\\n]+"
                }
            }],
            "tool_choice": {"type": "custom", "name": "PowerShell"}
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    assert_eq!(first.upstream_body()["tools"][0]["name"], "PowerShell");
    assert_eq!(
        first.upstream_body()["tools"][0]["input_schema"]["properties"]["input"]["type"],
        "string"
    );
    assert!(
        first.upstream_body()["tools"][0]["input_schema"]["properties"]["input"]["description"]
            .as_str()
            .unwrap()
            .contains("regex grammar")
    );
    assert_eq!(
        first.upstream_body()["tool_choice"],
        json!({"type": "tool", "name": "PowerShell"})
    );

    let response = translate_messages_response(
        first,
        &json!({
            "id": "msg_custom",
            "content": [{
                "type": "tool_use",
                "id": "toolu_custom",
                "name": "PowerShell",
                "input": {"input": "Get-ChildItem -Force"}
            }]
        }),
    )
    .unwrap();
    assert_eq!(
        response.response_body["output"][0]["type"],
        "custom_tool_call"
    );
    assert_eq!(
        response.response_body["output"][0]["input"],
        "Get-ChildItem -Force"
    );

    let second = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "previous_response_id": response.response_id,
            "input": [{
                "type": "custom_tool_call_output",
                "call_id": "toolu_custom",
                "output": "Cargo.toml\nsrc"
            }]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        Some(response.continuation),
    )
    .unwrap();
    assert_eq!(
        second.upstream_body()["messages"][2]["content"][0],
        json!({
            "type": "tool_result",
            "tool_use_id": "toolu_custom",
            "content": "Cargo.toml\nsrc"
        })
    );
}

#[test]
fn messages_bridge_rejects_non_text_custom_tool_output() {
    let first = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "input": "List the project files.",
            "tools": [{"type": "custom", "name": "PowerShell"}]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let response = translate_messages_response(
        first,
        &json!({
            "id": "msg_custom_output",
            "content": [{
                "type": "tool_use",
                "id": "toolu_custom_output",
                "name": "PowerShell",
                "input": {"input": "Get-ChildItem"}
            }]
        }),
    )
    .unwrap();

    let error = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "previous_response_id": response.response_id,
            "input": [{
                "type": "custom_tool_call_output",
                "call_id": "toolu_custom_output",
                "output": [{"type": "input_text", "text": "not a direct text result"}]
            }]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        Some(response.continuation),
    )
    .unwrap_err();

    assert_eq!(error.code(), "adapter_invalid_request");
}

#[test]
fn messages_bridge_preserves_allowed_tool_subset_without_lie() {
    let mut request = request(Value::String("choose".to_string()));
    request["tools"] = json!([
        {
            "type": "function",
            "name": "run_command",
            "parameters": {"type": "object"}
        },
        {
            "type": "function",
            "name": "read_file",
            "parameters": {"type": "object"}
        }
    ]);
    request["tool_choice"] = json!({
        "type": "allowed_tools",
        "mode": "required",
        "tools": [{"type": "function", "name": "run_command"}]
    });

    let prepared = prepare_responses_to_messages(
        &request,
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    assert_eq!(
        prepared.upstream_body()["tools"].as_array().unwrap().len(),
        1
    );
    assert_eq!(prepared.upstream_body()["tools"][0]["name"], "run_command");
    assert_eq!(prepared.upstream_body()["tool_choice"]["type"], "any");
}

#[test]
fn messages_bridge_rejects_an_upstream_tool_that_was_not_declared() {
    let prepared = prepare_responses_to_messages(
        &request(Value::String("choose".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let error = translate_messages_response(
        prepared,
        &json!({
            "id": "msg_unknown_tool",
            "content": [{
                "type": "tool_use",
                "id": "toolu_unknown",
                "name": "not_declared",
                "input": {}
            }]
        }),
    )
    .unwrap_err();
    assert_eq!(error.code(), "adapter_upstream_response_invalid");
}

#[test]
fn messages_bridge_preserves_text_and_tool_output_order() {
    let prepared = prepare_responses_to_messages(
        &request(Value::String("ordered".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let response = translate_messages_response(
        prepared,
        &json!({
            "id": "msg_ordered",
            "content": [
                {"type": "text", "text": "before"},
                {"type": "tool_use", "id": "tool_ordered", "name": "run_command", "input": {"command": "pwd"}},
                {"type": "text", "text": "after"}
            ]
        }),
    )
    .unwrap();
    let output = response.response_body["output"].as_array().unwrap();
    assert_eq!(output[0]["type"], "message");
    assert_eq!(output[1]["type"], "function_call");
    assert_eq!(output[2]["type"], "message");
    assert_ne!(output[0]["id"], output[2]["id"]);
}

#[test]
fn messages_bridge_rejects_tool_result_for_an_unknown_call() {
    let first = prepare_responses_to_messages(
        &request(Value::String("inspect the project".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let response = translate_messages_response(
        first,
        &json!({
            "id": "msg_01",
            "content": [{
                "type": "tool_use",
                "id": "toolu_01",
                "name": "run_command",
                "input": {"command": "pwd"}
            }]
        }),
    )
    .unwrap();

    let error = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "previous_response_id": response.response_id,
            "input": [{
                "type": "function_call_output",
                "call_id": "toolu_other",
                "output": "unexpected"
            }]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        Some(response.continuation),
    )
    .unwrap_err();

    assert_eq!(error.code(), "adapter_continuation_mismatch");
}

#[test]
fn messages_bridge_maps_case_insensitive_reasoning_only_when_binding_supports_it() {
    let mut with_reasoning = request(Value::String("think".to_string()));
    with_reasoning["reasoning"] = json!({"effort": "High"});
    assert!(MessagesReasoningMode::Adaptive.supports_effort("HIGH"));
    assert!(MessagesReasoningMode::Budget.supports_effort("High"));
    let adaptive = prepare_responses_to_messages(
        &with_reasoning,
        "claude-test",
        false,
        MessagesReasoningMode::Adaptive,
        None,
    )
    .unwrap();
    assert_eq!(adaptive.upstream_body()["thinking"]["type"], "adaptive");
    assert_eq!(adaptive.upstream_body()["output_config"]["effort"], "high");
    assert!(adaptive.upstream_body().get("temperature").is_none());

    let budget = prepare_responses_to_messages(
        &with_reasoning,
        "claude-test",
        false,
        MessagesReasoningMode::Budget,
        None,
    )
    .unwrap();
    assert_eq!(budget.upstream_body()["thinking"]["budget_tokens"], 16_384);

    let error = prepare_responses_to_messages(
        &with_reasoning,
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), "adapter_reasoning_unsupported");
}

#[test]
fn messages_bridge_rejects_hosted_tools_instead_of_lying_about_support() {
    let error = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "input": "hello",
            "tools": [{"type": "web_search"}]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap_err();
    assert_eq!(error.code(), "adapter_tool_unsupported");
}

#[test]
fn messages_bridge_keeps_client_tools_when_hosted_tools_are_also_present() {
    let prepared = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "input": "inspect",
            "tools": [
                {"type": "web_search"},
                {
                    "type": "function",
                    "name": "run_command",
                    "parameters": {"type": "object"}
                }
            ]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();

    assert_eq!(
        prepared.upstream_body()["tools"],
        json!([{
            "name": "run_command",
            "input_schema": {"type": "object"}
        }])
    );
}

#[test]
fn messages_stream_bridge_emits_responses_tool_events_and_completion() {
    let request = prepare_responses_to_messages(
        &request(Value::String("run pwd".to_string())),
        "claude-test",
        true,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let mut bridge = MessagesStreamBridge::new(request);
    bridge.push(
        br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_stream","usage":{"input_tokens":4,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_stream","name":"run_command","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"pwd\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":4,"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#,
    );
    let output = std::iter::from_fn(|| bridge.pop_output())
        .map(|frame| String::from_utf8(frame).unwrap())
        .collect::<Vec<_>>()
        .join("");
    assert!(output.contains("response.function_call_arguments.delta"));
    assert!(output.contains("response.function_call_arguments.done"));
    assert!(output.contains("response.output_item.done"));
    assert!(output.contains("response.completed"));
    assert_eq!(
        bridge.completed().unwrap().response_body["output"][0]["call_id"],
        "toolu_stream"
    );
}

#[test]
fn messages_stream_bridge_emits_custom_tool_events_and_completion() {
    let request = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "input": "List the project files.",
            "tools": [{"type": "custom", "name": "PowerShell"}]
        }),
        "claude-test",
        true,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let mut bridge = MessagesStreamBridge::new(request);
    bridge.push(
        br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_custom_stream","usage":{"input_tokens":4,"output_tokens":0}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"toolu_custom_stream","name":"PowerShell","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"input\":\"Get-ChildItem\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"input_tokens":4,"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#,
    );
    let output = std::iter::from_fn(|| bridge.pop_output())
        .map(|frame| String::from_utf8(frame).unwrap())
        .collect::<Vec<_>>()
        .join("");
    assert!(output.contains("response.custom_tool_call_input.done"));
    assert!(output.contains("\"type\":\"custom_tool_call\""));
    assert!(!output.contains("response.function_call_arguments.delta"));
    assert_eq!(
        bridge.completed().unwrap().response_body["output"][0]["type"],
        "custom_tool_call"
    );
    assert_eq!(
        bridge.completed().unwrap().response_body["output"][0]["input"],
        "Get-ChildItem"
    );
}

#[test]
fn scoped_bridge_ids_keep_same_upstream_id_isolated_between_routes() {
    let first = prepare_responses_to_messages_scoped(
        &request(Value::String("first".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "source-a/responses-bridge",
    )
    .unwrap();
    let second = prepare_responses_to_messages_scoped(
        &request(Value::String("second".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "source-b/responses-bridge",
    )
    .unwrap();
    let upstream = json!({
        "id": "msg_same",
        "content": [{"type": "text", "text": "ok"}]
    });
    let first = translate_messages_response(first, &upstream).unwrap();
    let second = translate_messages_response(second, &upstream).unwrap();

    assert_ne!(first.response_id, second.response_id);
    assert_eq!(
        first.response_body["output"][0]["id"],
        format!("msg_{}", first.response_id)
    );
    assert_eq!(
        second.response_body["output"][0]["id"],
        format!("msg_{}", second.response_id)
    );
}

#[test]
fn messages_stream_bridge_emits_text_done_and_accepts_metadata_events() {
    let request = prepare_responses_to_messages(
        &request(Value::String("say hi".to_string())),
        "claude-test",
        true,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let mut bridge = MessagesStreamBridge::new(request);
    bridge.push(
        br#"event: message_start
data: {"type":"message_start","message":{"id":"msg_text","usage":{"input_tokens":1}}}

event: ping
data: {"type":"ping"}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"citations_delta","citation":{"type":"char_location"}}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_stop
data: {"type":"message_stop"}

"#,
    );
    let output = std::iter::from_fn(|| bridge.pop_output())
        .map(|frame| String::from_utf8(frame).unwrap())
        .collect::<Vec<_>>()
        .join("");

    assert!(bridge.completed().is_some());
    assert!(output.contains("response.output_text.delta"));
    assert!(output.contains("response.output_text.done"));
    assert!(output.contains("\"text\":\"hello\""));
}

#[test]
fn messages_stream_bridge_keeps_text_tool_text_order_and_response_item_ids() {
    let request = prepare_responses_to_messages(
        &request(Value::String("inspect then summarize".to_string())),
        "claude-test",
        true,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let mut bridge = MessagesStreamBridge::new(request);
    bridge.push(
        br#"data: {"type":"message_start","message":{"id":"msg_interleaved"}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":"before"}}

data: {"type":"content_block_stop","index":0}

data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_interleaved","name":"run_command","input":{}}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":\"pwd\"}"}}

data: {"type":"content_block_stop","index":1}

data: {"type":"content_block_start","index":2,"content_block":{"type":"text"}}

data: {"type":"content_block_delta","index":2,"delta":{"type":"text_delta","text":"after"}}

data: {"type":"content_block_stop","index":2}

data: {"type":"message_stop"}

"#,
    );

    let frames = std::iter::from_fn(|| bridge.pop_output())
        .map(|frame| {
            let frame = String::from_utf8(frame).unwrap();
            let data = frame
                .lines()
                .find_map(|line| line.strip_prefix("data: "))
                .expect("bridge frames contain JSON data");
            serde_json::from_str::<Value>(data).unwrap()
        })
        .collect::<Vec<_>>();
    let completed = bridge.completed().expect("interleaved stream completes");
    let output = completed.response_body["output"].as_array().unwrap();
    assert_eq!(
        output
            .iter()
            .map(|item| item["type"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["message", "function_call", "message"]
    );
    assert_eq!(output[0]["content"][0]["text"], "before");
    assert_eq!(output[1]["call_id"], "toolu_interleaved");
    assert_eq!(output[2]["content"][0]["text"], "after");
    assert_ne!(output[0]["id"], output[2]["id"]);

    let completed_items = frames
        .iter()
        .filter(|frame| frame["type"] == "response.output_item.done")
        .collect::<Vec<_>>();
    assert_eq!(completed_items.len(), 3);
    assert_eq!(completed_items[0]["output_index"], 0);
    assert_eq!(completed_items[0]["item"]["id"], output[0]["id"]);
    assert_eq!(completed_items[1]["output_index"], 1);
    assert_eq!(completed_items[1]["item"]["id"], output[1]["id"]);
    assert_eq!(completed_items[2]["output_index"], 2);
    assert_eq!(completed_items[2]["item"]["id"], output[2]["id"]);

    let continuation = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "previous_response_id": completed.response_id,
            "input": [{
                "type": "function_call_output",
                "call_id": "toolu_interleaved",
                "output": "/workspace"
            }]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        Some(completed.continuation.clone()),
    )
    .unwrap();
    assert_eq!(
        continuation.upstream_body()["messages"][2]["content"][0]["tool_use_id"],
        "toolu_interleaved"
    );
}

#[test]
fn messages_stream_bridge_rejects_unclosed_blocks_before_message_stop() {
    let request = prepare_responses_to_messages(
        &request(Value::String("say hi".to_string())),
        "claude-test",
        true,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let mut bridge = MessagesStreamBridge::new(request);
    bridge.push(
        br#"data: {"type":"message_start","message":{"id":"msg_incomplete"}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"text"}}

data: {"type":"message_stop"}

"#,
    );

    assert!(bridge.is_terminal());
    assert!(bridge.completed().is_none());
    let output = std::iter::from_fn(|| bridge.pop_output())
        .map(|frame| String::from_utf8(frame).unwrap())
        .collect::<Vec<_>>()
        .join("");
    assert!(output.contains("response.failed"));
}

#[test]
fn messages_stream_bridge_preserves_initial_and_incremental_thinking_signature() {
    let request = prepare_responses_to_messages(
        &request(Value::String("think".to_string())),
        "claude-test",
        true,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let mut bridge = MessagesStreamBridge::new(request);
    bridge.push(
        br#"data: {"type":"message_start","message":{"id":"msg_thinking"}}

data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"initial ","signature":"sig-"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"more"}}

data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"part"}}

data: {"type":"content_block_stop","index":0}

data: {"type":"content_block_start","index":1,"content_block":{"type":"text"}}

data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"done"}}

data: {"type":"content_block_stop","index":1}

data: {"type":"message_stop"}

"#,
    );

    let completed = bridge.completed().expect("thinking stream should complete");
    let messages = &completed.continuation.messages;
    assert_eq!(messages[1]["content"][0]["thinking"], "initial more");
    assert_eq!(messages[1]["content"][0]["signature"], "sig-part");
}

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
    assert_eq!(replayed["tools"], initial["tools"]);
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
    assert_eq!(input[1]["id"], "fc_function_01");
    assert_eq!(input[1]["call_id"], "call_function_01");
    assert_eq!(input[2]["id"], "fc_function_02");
    assert_eq!(input[3]["id"], "call_custom_01");
    assert_eq!(input[4]["call_id"], "call_function_01");
    assert!(!repair_call_prefixed_function_item_ids(&mut request));
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
