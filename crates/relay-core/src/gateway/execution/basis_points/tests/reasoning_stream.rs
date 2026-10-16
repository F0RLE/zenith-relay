use super::*;
use crate::{AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter, WireApi};

fn response() -> Value {
    json!({"id":"resp_synthetic","status":"completed","output":[
        {"type":"reasoning","id":"rs_synthetic","summary":[
            {"type":"summary_text","text":"Plan the edit."},
            {"type":"summary_text","text":"Check the result."}
        ],"encrypted_content":"synthetic-opaque-state"},
        {"type":"function_call","id":"fc_synthetic","call_id":"call_edit","name":"edit_file","arguments":"{\"path\":\"synthetic.txt\"}","status":"completed"}
    ],"usage":{"input_tokens":10,"output_tokens":8,"output_tokens_details":{"reasoning_tokens":5}}})
}

fn assert_messages_stream(bytes: &[u8], native: &Value, expected: Value) {
    let request = json!({"messages":[{"role":"user","content":"Update the synthetic file."}],
        "max_tokens":4096,"thinking":{"type":"adaptive"},"output_config":{"effort":"high"},
        "tools":[{"name":"edit_file","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}]});
    let mut bridge = SourceAdapter::MessagesToResponses
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Messages,
            request: &request,
            model: "synthetic-model",
            stream: true,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: None,
            response_scope: "synthetic-scope",
            response_id_seed: "synthetic-seed",
        })
        .unwrap()
        .into_stream_bridge()
        .unwrap();
    // Exercise arbitrary transport boundaries rather than only whole SSE frames.
    for chunk in bytes.chunks(13) {
        bridge.push(chunk);
    }
    bridge.finish();
    let frames = String::from_utf8(
        std::iter::from_fn(|| bridge.pop_output())
            .flatten()
            .collect(),
    )
    .unwrap();
    assert!(!frames.contains("event: error"), "{frames}");
    let completed = bridge
        .completed()
        .expect("BPS reasoning must not abort tool delivery");
    let body = &completed.response_body;
    assert_eq!(body["content"], expected);
    assert_eq!(body["stop_reason"], "tool_use");
    assert_eq!(body["usage"]["output_tokens"], 8);
    let mut next = request;
    next["messages"].as_array_mut().unwrap().extend([
        json!({"role":"assistant","content":body["content"]}),
        json!({"role":"user","content":[{"type":"tool_result","tool_use_id":"call_edit","content":"updated"}]}),
    ]);
    let continued = SourceAdapter::MessagesToResponses
        .prepare_request(AdapterRequestContext {
            client_wire_api: WireApi::Messages,
            request: &next,
            model: "synthetic-model",
            stream: false,
            reasoning_mode: MessagesReasoningMode::Adaptive,
            cache_write_ttl: CacheWriteTtl::Provider,
            previous: Some(completed.continuation.clone()),
            response_scope: "synthetic-scope",
            response_id_seed: "next",
        })
        .unwrap();
    let native_items = native["output"].as_array().unwrap();
    let continued_input = continued.upstream_body()["input"].as_array().unwrap();
    assert_eq!(&continued_input[1..=native_items.len()], native_items);
    assert_eq!(
        continued_input[native_items.len() + 1]["call_id"],
        "call_edit"
    );
    assert!(!frames.contains("synthetic-opaque-state"));
    assert!(!frames.contains("signature_delta"));
    assert_eq!(frames.matches("event: message_stop").count(), 1);
}

fn expected_content() -> Value {
    json!([
        {"type":"thinking","thinking":"Plan the edit."},
        {"type":"thinking","thinking":"Check the result."},
        {"type":"tool_use","id":"call_edit","name":"edit_file","input":{"path":"synthetic.txt"}}
    ])
}

#[test]
fn synthetic_bps_reasoning_stream_reaches_messages_with_edit_tool() {
    let native = response();
    let bytes = synthetic_stream(&serde_json::to_vec(&native).unwrap()).unwrap();
    assert_messages_stream(&bytes, &native, expected_content());
}

#[test]
fn withheld_bps_reasoning_stream_reaches_messages_with_edit_tool() {
    let native = response();
    let mut bytes = b"data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_synthetic\",\"output\":[]}}\n\n".to_vec();
    response::stream::emit_unseen_output(&native, &Default::default(), |event| {
        bytes.extend(format!("data: {event}\n\n").as_bytes())
    });
    bytes.extend(
        format!(
            "data: {}\n\n",
            json!({"type":"response.completed","response":native})
        )
        .as_bytes(),
    );
    assert_messages_stream(&bytes, &native, expected_content());
}

#[test]
fn bps_reasoning_stream_preserves_text_order_and_opaque_only_tool_replay() {
    for text_position in [None, Some(0), Some(1)] {
        let mut native = response();
        let mut expected = expected_content();
        if let Some(position) = text_position {
            native["output"].as_array_mut().unwrap().insert(
                position,
                json!({
                    "type":"message","id":"msg_synthetic","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":"Updating.","annotations":[]}]
                }),
            );
            let position = if position == 0 { 0 } else { 2 };
            expected.as_array_mut().unwrap().insert(
                position,
                json!({
                    "type":"text","text":"Updating."
                }),
            );
        } else {
            native["output"][0]["summary"] = json!([]);
            expected.as_array_mut().unwrap().drain(..2);
        }
        let bytes = synthetic_stream(&serde_json::to_vec(&native).unwrap()).unwrap();
        assert_messages_stream(&bytes, &native, expected);
    }
}
