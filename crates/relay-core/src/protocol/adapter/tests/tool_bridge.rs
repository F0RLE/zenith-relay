use super::*;
use serde_json::json;

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
            "stop_reason": "tool_use",
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
    assert_eq!(
        response.response_body["output"][0]["id"],
        "ctc_toolu_custom"
    );
    assert_eq!(
        response.response_body["output"][0]["call_id"],
        "toolu_custom"
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
fn messages_bridge_converts_multimodal_function_output_to_anthropic_blocks() {
    let first = prepare_responses_to_messages(
        &request(Value::String("inspect the image".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let response = translate_messages_response(
        first,
        &json!({
            "id": "msg_image",
            "stop_reason": "tool_use",
            "content": [{
                "type": "tool_use",
                "id": "toolu_image",
                "name": "run_command",
                "input": {"command": "view-image"}
            }]
        }),
    )
    .unwrap();

    let continued = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "previous_response_id": response.response_id,
            "input": [{
                "type": "function_call_output",
                "call_id": "toolu_image",
                "output": [
                    {"type": "input_text", "text": "image loaded"},
                    {"type": "input_image", "image_url": "data:image/png;base64,YQ=="}
                ]
            }]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        Some(response.continuation),
    )
    .unwrap();

    assert_eq!(
        continued.upstream_body()["messages"][2]["content"][0]["content"],
        json!([
            {"type": "text", "text": "image loaded"},
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/png",
                    "data": "YQ=="
                }
            }
        ])
    );
}
#[test]
fn messages_bridge_keeps_regular_function_output_arrays_as_json_text() {
    let first = prepare_responses_to_messages(
        &request(Value::String("inspect".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let response = translate_messages_response(
        first,
        &json!({
            "id": "msg_json_output",
            "stop_reason": "tool_use",
            "content": [{
                "type": "tool_use",
                "id": "toolu_json_output",
                "name": "run_command",
                "input": {"command": "list"}
            }]
        }),
    )
    .unwrap();
    let continued = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "previous_response_id": response.response_id,
            "input": [{
                "type": "function_call_output",
                "call_id": "toolu_json_output",
                "output": [{"name": "Cargo.toml"}, {"name": "README.md"}]
            }]
        }),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        Some(response.continuation),
    )
    .unwrap();

    assert_eq!(
        continued.upstream_body()["messages"][2]["content"][0]["content"],
        r#"[{"name":"Cargo.toml"},{"name":"README.md"}]"#
    );
}
#[test]
fn messages_bridge_rejects_invalid_tool_output_image_data() {
    let first = prepare_responses_to_messages(
        &request(Value::String("inspect".to_string())),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();
    let response = translate_messages_response(
        first,
        &json!({
            "id": "msg_invalid_image",
            "stop_reason": "tool_use",
            "content": [{
                "type": "tool_use",
                "id": "toolu_invalid_image",
                "name": "run_command",
                "input": {"command": "view-image"}
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
                "call_id": "toolu_invalid_image",
                "output": [{
                    "type": "input_image",
                    "image_url": "data:image/png;base64,not-valid-base64"
                }]
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
            "stop_reason": "tool_use",
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
            "stop_reason": "tool_use",
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
            "stop_reason": "tool_use",
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
            "stop_reason": "tool_use",
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
fn messages_bridge_rejects_hosted_tools_before_sending_the_request() {
    let error = prepare_responses_to_messages(
        &json!({
            "model": "claude-test",
            "input": "hello",
            "tools": [{"type": "web_search"}],
            "tool_choice": {"type": "function", "name": "missing"}
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
fn messages_bridge_rejects_mixed_hosted_and_client_tools_before_sending_the_request() {
    let error = prepare_responses_to_messages(
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
    .unwrap_err();
    assert_eq!(error.code(), "adapter_tool_unsupported");
}
