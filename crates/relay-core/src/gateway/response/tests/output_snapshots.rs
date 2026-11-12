use super::*;
use serde_json::json;

fn call() -> Value {
    json!({"type":"function_call","id":"fc_synthetic","call_id":"call_synthetic",
        "name":"lookup","arguments":"{}","status":"completed"})
}

fn done(index: Option<Value>, item: Value) -> Value {
    let mut event = json!({"type":"response.output_item.done","item":item});
    if let Some(index) = index {
        event["output_index"] = index;
    }
    event
}

fn buffered(events: Vec<Value>, output: Value) -> Result<Value, ()> {
    let terminal = json!({"type":"response.completed","response":{
        "id":"resp_synthetic","status":"completed","output":output}});
    let bytes = events
        .into_iter()
        .chain([terminal])
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    completed_upstream_response(bytes.as_bytes(), true, None)
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
        .map_err(|_| ())
}

#[test]
fn buffered_sparse_output_orders_indices_and_deduplicates_tool_calls() {
    let reasoning = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
        "encrypted_content":"synthetic-cipher","status":"completed"});
    let response = buffered(
        vec![
            done(Some(json!(1)), call()),
            done(Some(json!(0)), reasoning.clone()),
            done(Some(json!(1)), call()),
        ],
        json!([]),
    )
    .unwrap();
    assert_eq!(response["output"], json!([reasoning, call()]));
}

#[test]
fn buffered_sparse_output_rejects_gaps_mixed_and_malformed_indices() {
    for events in [
        vec![done(Some(json!(1)), call())],
        vec![done(Some(json!(0)), call()), done(Some(json!(2)), call())],
        vec![done(None, call()), done(Some(json!(0)), call())],
        vec![done(Some(json!(-1)), call())],
        vec![done(Some(json!("0")), call())],
        vec![done(Some(Value::Null), call())],
    ] {
        assert!(buffered(events, json!([])).is_err());
    }
}

#[test]
fn buffered_duplicate_index_cannot_overwrite_a_completed_tool_call() {
    for (field, value) in [
        ("call_id", json!("call_other")),
        ("arguments", json!("{\"changed\":true}")),
        ("name", json!("other_tool")),
    ] {
        let mut changed = call();
        changed[field] = value;
        assert!(
            buffered(
                vec![done(Some(json!(0)), call()), done(Some(json!(0)), changed)],
                json!([]),
            )
            .is_err(),
            "conflicting completed {field}"
        );
    }
}

#[test]
fn buffered_unindexed_items_keep_arrival_order_and_full_terminal_fills_gaps() {
    let message = json!({"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"synthetic"}]});
    let expected = json!([message, call()]);
    let response = buffered(vec![done(None, message), done(None, call())], json!([])).unwrap();
    assert_eq!(response["output"], expected);
    let response = buffered(vec![done(Some(json!(1)), call())], expected.clone()).unwrap();
    assert_eq!(response["output"], expected);
}

#[test]
fn buffered_terminal_output_cannot_replace_completed_items() {
    for (item, field, replacement) in [
        (call(), "call_id", json!("call_other")),
        (call(), "arguments", json!("{\"changed\":true}")),
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
        let mut terminal = item.clone();
        terminal[field] = replacement;
        assert!(
            buffered(vec![done(Some(json!(0)), item)], json!([terminal])).is_err(),
            "terminal changed completed {field}"
        );
    }
}

#[test]
fn buffered_terminal_output_cannot_hide_conflicting_completed_snapshots() {
    let mut changed = call();
    changed["arguments"] = json!("{\"changed\":true}");
    for terminal in [call(), changed.clone()] {
        assert!(
            buffered(
                vec![
                    done(Some(json!(0)), call()),
                    done(Some(json!(0)), changed.clone()),
                ],
                json!([terminal]),
            )
            .is_err()
        );
    }
}

#[test]
fn buffered_terminal_output_preserves_unindexed_and_nullable_compatibility() {
    let message = json!({"type":"message","role":"assistant",
        "content":[{"type":"output_text","text":"synthetic"}]});
    let mut terminal_message = message.clone();
    terminal_message["phase"] = Value::Null;
    let expected = json!([terminal_message, call()]);
    let response = buffered(
        vec![done(None, message), done(None, call())],
        expected.clone(),
    )
    .unwrap();
    assert_eq!(response["output"], expected);
    let response = buffered(vec![done(None, call())], expected.clone()).unwrap();
    assert_eq!(response["output"], expected);
}
