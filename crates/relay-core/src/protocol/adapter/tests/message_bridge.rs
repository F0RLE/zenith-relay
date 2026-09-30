use super::*;
use serde_json::json;

#[test]
fn messages_cache_write_lifetime_overrides_existing_markers() {
    let mut request = json!({
        "system": [{"type": "text", "text": "system"}],
        "messages": [{"role": "user", "content": [{"type": "text", "text": "hello", "cache_control": {"type": "ephemeral", "ttl": "5m"}}]}]
    });

    messages::apply_cache_write_ttl(&mut request, CacheWriteTtl::OneHour).unwrap();

    assert_eq!(
        request["messages"][0]["content"][0]["cache_control"]["ttl"],
        "1h"
    );
}
#[test]
fn messages_cache_write_lifetime_marks_stable_prefix_and_latest_turn() {
    let mut request = json!({
        "system": [{"type": "text", "text": "stable system"}],
        "tools": [{"name": "lookup", "description": "stable tool", "input_schema": {"type": "object"}}],
        "messages": [
            {"role": "user", "content": [{"type": "text", "text": "older turn"}]},
            {"role": "user", "content": [{"type": "text", "text": "latest turn"}]}
        ]
    });

    messages::apply_cache_write_ttl(&mut request, CacheWriteTtl::OneHour).unwrap();

    assert_eq!(request["system"][0]["cache_control"]["ttl"], "1h");
    assert!(request["tools"][0].get("cache_control").is_none());
    assert!(request["messages"][0]["content"][0]
        .get("cache_control")
        .is_none());
    assert_eq!(
        request["messages"][1]["content"][0]["cache_control"]["ttl"],
        "1h"
    );
}
#[test]
fn messages_bridge_converts_user_image_input_to_anthropic_blocks() {
    let prepared = prepare_responses_to_messages(
        &request(json!([{
            "type": "message",
            "role": "user",
            "content": [
                {"type": "input_text", "text": "What is in this image?"},
                {"type": "input_image", "image_url": "data:image/jpeg;base64,YQ=="}
            ]
        }])),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap();

    assert_eq!(
        prepared.upstream_body()["messages"][0]["content"],
        json!([
            {"type": "text", "text": "What is in this image?"},
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/jpeg",
                    "data": "YQ=="
                }
            }
        ])
    );
}
#[test]
fn messages_bridge_rejects_invalid_user_image_input() {
    let error = prepare_responses_to_messages(
        &request(json!([{
            "type": "message",
            "role": "user",
            "content": [{
                "type": "input_image",
                "image_url": "data:image/png;base64,not-valid-base64"
            }]
        }])),
        "claude-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
    )
    .unwrap_err();

    assert_eq!(error.code(), "adapter_invalid_request");
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
    assert_eq!(
        bridge.completed().unwrap().response_body["output"][0]["id"],
        "ctc_toolu_custom_stream"
    );
    assert_eq!(
        bridge.completed().unwrap().response_body["output"][0]["call_id"],
        "toolu_custom_stream"
    );
}
#[test]
fn messages_bridge_preserves_output_limit_and_rejects_unknown_terminal_reason() {
    let request = || {
        prepare_responses_to_messages(
            &request(Value::String("continue".to_string())),
            "claude-test",
            false,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap()
    };
    let limited = translate_messages_response(
        request(),
        &json!({"id":"msg_limited","stop_reason":"max_tokens","content":[{"type":"text","text":"partial"}]}),
    )
    .unwrap();
    assert_eq!(limited.response_body["status"], "incomplete");
    assert_eq!(
        limited.response_body["incomplete_details"]["reason"],
        "max_output_tokens"
    );
    for reason in [None, Some("pause_turn")] {
        let mut upstream = json!({"id":"msg_unknown","content":[{"type":"text","text":"partial"}]});
        if let Some(reason) = reason {
            upstream["stop_reason"] = reason.into();
        }
        assert_eq!(
            translate_messages_response(request(), &upstream)
                .unwrap_err()
                .code(),
            "adapter_upstream_response_invalid"
        );
    }
}
#[test]
fn messages_bridge_accepts_empty_refusal_in_json_and_stream() {
    let make_request = |stream| {
        prepare_responses_to_messages(
            &request(Value::String("hello".to_string())),
            "claude-test",
            stream,
            MessagesReasoningMode::Disabled,
            None,
        )
        .unwrap()
    };
    let json = translate_messages_response(
        make_request(false),
        &json!({"id":"msg_empty_refusal","stop_reason":"refusal","content":[]}),
    )
    .unwrap();
    assert_eq!(json.response_body["status"], "incomplete");
    assert_eq!(json.response_body["output"], json!([]));
    assert_eq!(
        json.response_body["incomplete_details"]["reason"],
        "content_filter"
    );

    let mut stream = MessagesStreamBridge::new(make_request(true));
    for event in [
        json!({"type":"message_start","message":{"id":"msg_empty_refusal","usage":{"input_tokens":2}}}),
        json!({"type":"message_delta","delta":{"stop_reason":"refusal"},"usage":{"output_tokens":1}}),
        json!({"type":"message_stop"}),
    ] {
        stream.push(format!("data: {event}\n\n").as_bytes());
    }
    let completed = stream.completed().expect("empty refusal should complete");
    assert_eq!(completed.response_body["status"], "incomplete");
    assert_eq!(completed.response_body["output"], json!([]));
    assert_eq!(
        completed.response_body["incomplete_details"]["reason"],
        "content_filter"
    );
    let events = std::iter::from_fn(|| stream.pop_output())
        .map(|chunk| String::from_utf8(chunk).unwrap())
        .collect::<String>();
    assert!(events.contains("response.incomplete"));
    assert!(!events.contains("response.failed"));
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

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}

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

data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}

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

data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}

data: {"type":"message_stop"}

"#,
    );

    let completed = bridge.completed().expect("thinking stream should complete");
    let messages = &completed.continuation.messages;
    assert_eq!(messages[1]["content"][0]["thinking"], "initial more");
    assert_eq!(messages[1]["content"][0]["signature"], "sig-part");
}
