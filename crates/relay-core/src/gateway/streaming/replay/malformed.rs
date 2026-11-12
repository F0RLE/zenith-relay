use super::*;

fn capture() -> NativeReplayCapture {
    let mut capture = NativeReplayCapture::default();
    capture.observe(&json!({"type":"response.output_item.done","output_index":0,
        "item":{"type":"message","id":"msg_synthetic","role":"assistant",
            "content":[{"type":"output_text","text":"synthetic"}]}}));
    capture
}

#[test]
fn malformed_terminal_output_cannot_be_repaired_into_a_checkpoint() {
    for output in [Value::Null, json!({}), json!(false), json!("invalid")] {
        assert!(
            capture()
                .finish(Some(json!({"id":"resp_synthetic","output":output})), None)
                .is_none()
        );
    }
}

#[test]
fn conflicting_or_invalid_terminal_identity_cannot_use_observed_identity() {
    for id in [
        Value::Null,
        json!(false),
        json!(42),
        json!(""),
        json!(" "),
        json!("resp_other"),
    ] {
        assert!(
            capture()
                .finish(Some(json!({"id":id,"output":[]})), Some("resp_synthetic"))
                .is_none()
        );
    }
}

#[test]
fn omitted_terminal_fields_can_use_completed_items_and_observed_identity() {
    for terminal in [
        None,
        Some(json!({})),
        Some(json!({"output":[]})),
        Some(json!({"id":"resp_synthetic"})),
    ] {
        let response = capture().finish(terminal, Some("resp_synthetic")).unwrap();
        assert_eq!(response["id"], "resp_synthetic");
        assert_eq!(response["output"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn conflicting_completed_items_cannot_be_overwritten_into_valid_replay() {
    let item = json!({"type":"function_call","id":"fc_synthetic","call_id":"call_synthetic",
        "name":"lookup","arguments":"{}","status":"completed"});
    for (field, value) in [
        ("id", json!("fc_other")),
        ("call_id", json!("call_other")),
        ("arguments", json!("{\"changed\":true}")),
        ("name", json!("other_tool")),
        ("status", json!("in_progress")),
        ("synthetic_extension", json!(true)),
    ] {
        let mut capture = NativeReplayCapture::default();
        let original = json!({"type":"response.output_item.done","output_index":0,"item":item});
        capture.observe(&original);
        let mut changed = original.clone();
        changed["item"][field] = value;
        capture.observe(&changed);
        capture.observe(&original);
        assert!(
            capture
                .finish(Some(json!({"id":"resp_synthetic","output":[item]})), None)
                .is_none(),
            "conflicting {field} cannot be repaired by a later snapshot"
        );
    }
}

#[test]
fn replay_terminal_output_cannot_replace_completed_items() {
    for (item, field, replacement) in [
        (
            json!({"type":"function_call","id":"fc_synthetic","call_id":"call_synthetic",
                "name":"lookup","arguments":"{}","status":"completed"}),
            "arguments",
            json!("{\"changed\":true}"),
        ),
        (
            json!({"type":"message","id":"msg_synthetic","role":"assistant",
                "content":[{"type":"output_text","text":"original"}]}),
            "content",
            json!([{"type":"output_text","text":"changed"}]),
        ),
        (
            json!({"type":"reasoning","id":"rs_synthetic","summary":[],
                "encrypted_content":"synthetic-cipher"}),
            "summary",
            json!([{"type":"summary_text","text":"changed"}]),
        ),
    ] {
        let mut capture = NativeReplayCapture::default();
        capture.observe(&json!({"type":"response.output_item.done","output_index":0,"item":item}));
        let mut terminal = item;
        terminal[field] = replacement;
        assert!(
            capture
                .finish(
                    Some(json!({"id":"resp_synthetic","output":[terminal]})),
                    None
                )
                .is_none(),
            "terminal changed completed {field}"
        );
    }
}

#[test]
fn replay_terminal_output_cannot_drop_a_completed_item() {
    let mut capture = NativeReplayCapture::default();
    let first = json!({"type":"message","id":"msg_first","role":"assistant","content":[]});
    let second = json!({"type":"message","id":"msg_second","role":"assistant","content":[]});
    capture.observe(&json!({"type":"response.output_item.done","output_index":0,"item":first}));
    capture.observe(&json!({"type":"response.output_item.done","output_index":1,"item":second}));
    assert!(
        capture
            .finish(Some(json!({"id":"resp_synthetic","output":[first]})), None)
            .is_none()
    );
}
