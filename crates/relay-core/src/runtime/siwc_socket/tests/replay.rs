use super::*;
use crate::usage::FullRequestReason;

mod nullable;

fn checkpoint(body: &Value, output: &Value) -> planner::Checkpoint {
    plan(body, None)
        .state
        .complete(&json!({"id":"first","status":"completed","output":output}))
        .unwrap()
}

fn assert_full_history(body: &Value, checkpoint: &planner::Checkpoint) {
    let planned = plan(body, Some(checkpoint));
    assert_eq!(
        planned.full_request_reason,
        Some(FullRequestReason::SocketOutputChanged)
    );
    let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
    assert_eq!(encoded["input"], body["input"]);
    assert!(encoded.get("previous_response_id").is_none());
}

#[test]
fn codex_reasoning_null_content_keeps_exact_socket_continuation() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let reasoning = json!({"type":"reasoning","id":"rs_test","status":"completed",
        "summary":[],"encrypted_content":"synthetic-cipher"});
    let checkpoint = checkpoint(&body, &json!([reasoning]));
    let mut replay = reasoning;
    replay.as_object_mut().unwrap().remove("status");
    replay["content"] = Value::Null;
    let new_input = json!({"role":"user","content":"synthetic next turn"});
    let mut next = body;
    next["input"]
        .as_array_mut()
        .unwrap()
        .extend([replay, new_input.clone()]);
    let planned = plan(&next, Some(&checkpoint));
    assert_eq!(planned.full_request_reason, None);
    let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
    assert_eq!(encoded["previous_response_id"], "first");
    assert_eq!(encoded["input"], json!([new_input]));
    for content in [
        json!([{"type":"reasoning_text","text":"changed"}]),
        json!([{"type":"text","text":"changed"}]),
        json!({}),
    ] {
        let mut changed = next.clone();
        changed["input"][1]["content"] = content;
        assert_full_history(&changed, &checkpoint);
    }
}

#[test]
fn empty_reasoning_content_matches_codex_omission_in_both_directions() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let reasoning = json!({"type":"reasoning","id":"rs_test","summary":[],
        "encrypted_content":"synthetic-cipher"});
    let mut empty = reasoning.clone();
    empty["content"] = json!([]);
    let mut null = reasoning.clone();
    null["content"] = Value::Null;
    for output in [&reasoning, &empty, &null] {
        let checkpoint = checkpoint(&body, &json!([output]));
        for replay in [&reasoning, &empty, &null] {
            let mut next = body.clone();
            let new_input = json!({"role":"user","content":"synthetic next"});
            next["input"]
                .as_array_mut()
                .unwrap()
                .extend([replay.clone(), new_input.clone()]);
            let planned = plan(&next, Some(&checkpoint));
            assert_eq!(planned.full_request_reason, None);
            let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "first");
            assert_eq!(encoded["input"], json!([new_input]));
            let full: Value = serde_json::from_str(&plan(&next, None).payload).unwrap();
            assert_eq!(full["input"], next["input"]);
        }
    }
    for output in [
        json!({"type":"message","id":"msg_test","role":"assistant","content":[]}),
        json!({"type":"synthetic_extension","content":[]}),
    ] {
        let checkpoint = checkpoint(&body, &json!([output]));
        let mut replay = output;
        replay.as_object_mut().unwrap().remove("content");
        let mut next = body.clone();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([replay, json!({"role":"user","content":"synthetic next"})]);
        assert_full_history(&next, &checkpoint);
    }
}

#[test]
fn documented_null_defaults_match_in_both_directions_without_rewriting_full_input() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for (item, field) in [
        (
            json!({"type":"message","id":"msg_test","role":"assistant",
            "status":"completed","content":[{"type":"output_text","text":"synthetic",
            "annotations":[],"logprobs":[]}]}),
            "phase",
        ),
        (
            json!({"type":"reasoning","id":"rs_test","summary":[]}),
            "encrypted_content",
        ),
        (
            json!({"type":"reasoning","id":"rs_test","summary":[],
            "encrypted_content":"synthetic-cipher"}),
            "content",
        ),
        (
            json!({"type":"function_call","id":"fc_test","call_id":"call_test",
            "name":"lookup","arguments":"{}","status":"completed"}),
            "caller",
        ),
        (
            json!({"type":"custom_tool_call","id":"ct_test","call_id":"call_test",
            "name":"lookup","input":"synthetic","status":"completed"}),
            "caller",
        ),
    ] {
        let mut null = item.clone();
        null[field] = Value::Null;
        for (output, replay) in [(&item, &null), (&null, &item)] {
            let checkpoint = checkpoint(&body, &json!([output]));
            let mut next = body.clone();
            let new_input = json!({"role":"user","content":"synthetic next turn"});
            next["input"]
                .as_array_mut()
                .unwrap()
                .extend([replay.clone(), new_input.clone()]);
            let encoded: Value =
                serde_json::from_str(&plan(&next, Some(&checkpoint)).payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "first", "{field}");
            assert_eq!(encoded["input"], json!([new_input]));
            let full: Value = serde_json::from_str(&plan(&next, None).payload).unwrap();
            assert_eq!(full["input"], next["input"]);
            let mut changed = next;
            changed["input"][1][field] = json!("synthetic-different-value");
            assert_full_history(&changed, &checkpoint);
        }
    }
}

#[test]
fn replay_normalization_keeps_identity_content_execution_and_extensions_strict() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for (item, field, replacement) in [
        (
            json!({"type":"reasoning","id":"rs_test","summary":[],
            "encrypted_content":"synthetic-cipher"}),
            "encrypted_content",
            Value::Null,
        ),
        (
            json!({"type":"reasoning","summary":[],"content":[
            {"type":"reasoning_text","text":"synthetic"}]}),
            "content",
            Value::Null,
        ),
        (
            json!({"type":"message","id":"msg_test","role":"assistant","phase":"final_answer",
            "content":[{"type":"output_text","text":"synthetic"}]}),
            "phase",
            Value::Null,
        ),
        (
            json!({"type":"function_call","id":"fc_test","call_id":"call_test",
            "name":"lookup","arguments":"{}","status":"completed",
            "caller":{"type":"program","caller_id":"synthetic"}}),
            "caller",
            Value::Null,
        ),
        (
            json!({"type":"function_call","id":"fc_test","call_id":"call_test",
            "name":"lookup","arguments":"{}","status":"completed"}),
            "status",
            Value::Null,
        ),
        (
            json!({"type":"reasoning","id":"rs_test","summary":[]}),
            "id",
            Value::Null,
        ),
        (
            json!({"type":"reasoning","summary":[]}),
            "summary",
            Value::Null,
        ),
        (
            json!({"type":"reasoning","summary":[]}),
            "synthetic_extension",
            Value::Null,
        ),
        (
            json!({"type":"function_call","id":"fc_test","call_id":"call_test",
            "name":"lookup","arguments":"{}","status":"completed"}),
            "call_id",
            json!("call_other"),
        ),
    ] {
        let checkpoint = checkpoint(&body, &json!([item]));
        let mut changed = item;
        changed[field] = replacement;
        let mut next = body.clone();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([changed, json!({"role":"user","content":"synthetic next"})]);
        assert_full_history(&next, &checkpoint);
    }
}

#[test]
fn replay_comparison_preserves_nested_content_and_array_order() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let message = json!({"type":"message","id":"msg_test","status":"completed",
        "phase":null,"role":"assistant","content":[{"type":"output_text","text":"synthetic",
        "annotations":[],"logprobs":[]}], "synthetic_extension":null});
    let reasoning = json!({"type":"reasoning","id":"rs_test","summary":[],
        "encrypted_content":"synthetic-cipher"});
    let checkpoint = checkpoint(&body, &json!([reasoning, message]));
    let mut next = body;
    next["input"].as_array_mut().unwrap().extend([
        reasoning,
        message,
        json!({"role":"user","content":"synthetic next"}),
    ]);
    for field in ["annotations", "logprobs", "text"] {
        let mut changed = next.clone();
        changed["input"][2]["content"][0]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert_full_history(&changed, &checkpoint);
    }
    let mut changed = next.clone();
    changed["input"][2]
        .as_object_mut()
        .unwrap()
        .remove("synthetic_extension");
    assert_full_history(&changed, &checkpoint);
    next["input"].as_array_mut().unwrap().swap(1, 2);
    assert_full_history(&next, &checkpoint);
}

#[test]
fn client_metadata_changes_preserve_continuation_without_rewriting_input() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for output in [
        json!({"type":"message","id":"msg_test","role":"assistant",
            "content":[{"type":"output_text","text":"synthetic"}]}),
        json!({"type":"reasoning","id":"rs_test","summary":[],
            "encrypted_content":"synthetic-cipher"}),
        json!({"type":"function_call","id":"fc_test","call_id":"call_test",
            "name":"lookup","arguments":"{}"}),
        json!({"type":"custom_tool_call","id":"ct_test","call_id":"call_test",
            "name":"lookup","input":"synthetic"}),
    ] {
        let metadata = json!({"turn_id":"synthetic-turn","create_time":1,
            "content_token_count":10,"content_item_kinds":[],"content_item_metadata":[]});
        let mut annotated = output.clone();
        annotated["internal_chat_message_metadata_passthrough"] = metadata;
        for (previous, replay) in [(&output, &annotated), (&annotated, &output)] {
            let checkpoint = checkpoint(&body, &json!([previous]));
            let next_item = json!({"role":"user","content":"synthetic next"});
            let mut next = body.clone();
            next["input"]
                .as_array_mut()
                .unwrap()
                .extend([replay.clone(), next_item.clone()]);
            let planned = plan(&next, Some(&checkpoint));
            assert_eq!(planned.full_request_reason, None);
            let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "first");
            assert_eq!(encoded["input"], json!([next_item]));
            let full: Value = serde_json::from_str(&plan(&next, None).payload).unwrap();
            assert_eq!(full["input"], next["input"]);
        }
    }
}

#[test]
fn metadata_normalization_keeps_tool_records_extensions_and_content_strict() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let output = json!({"type":"message","id":"msg_test","role":"assistant",
        "content":[{"type":"output_text","text":"synthetic"}],
        "internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-before"}});
    let checkpoint = checkpoint(&body, &json!([output]));
    for metadata in [
        json!({"executed_tool_calls":[]}),
        json!({"tool_calls_complete":false}),
        json!({"cell_id":"synthetic-cell"}),
        json!({"synthetic_extension":null}),
        json!("malformed"),
    ] {
        let mut replay = output.clone();
        replay["internal_chat_message_metadata_passthrough"] = metadata;
        let mut next = body.clone();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([replay, json!({"role":"user","content":"synthetic next"})]);
        assert_full_history(&next, &checkpoint);
    }
    for kind in [
        "function_call_output",
        "custom_tool_call_output",
        "synthetic_extension",
    ] {
        let output = json!({"type":kind,"call_id":"call_test","output":"synthetic",
            "internal_chat_message_metadata_passthrough":{"turn_id":"synthetic-before"}});
        let checkpoint = self::checkpoint(&body, &json!([output]));
        let mut replay = output;
        replay["internal_chat_message_metadata_passthrough"]["turn_id"] = json!("synthetic-after");
        let mut next = body.clone();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([replay, json!({"role":"user","content":"synthetic next"})]);
        assert_full_history(&next, &checkpoint);
    }
    for (field, replacement) in [
        ("id", json!("msg_other")),
        (
            "content",
            json!([{"type":"output_text","text":"synthetic changed"}]),
        ),
    ] {
        let mut replay = output.clone();
        replay["internal_chat_message_metadata_passthrough"] = json!({"turn_id":"synthetic-after"});
        replay[field] = replacement;
        let mut next = body.clone();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([replay, json!({"role":"user","content":"synthetic next"})]);
        assert_full_history(&next, &checkpoint);
    }
}
