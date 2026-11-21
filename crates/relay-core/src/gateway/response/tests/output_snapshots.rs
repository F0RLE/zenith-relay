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
    buffered_wire(events, output)
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
        .map_err(|_| ())
}

fn buffered_wire(
    events: Vec<Value>,
    output: Value,
) -> Result<Vec<u8>, Box<crate::gateway::streaming::StreamBootstrapFailure>> {
    let terminal = json!({"type":"response.completed","response":{
        "id":"resp_synthetic","status":"completed","output":output}});
    let bytes = events
        .into_iter()
        .chain([terminal])
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    completed_upstream_response(bytes.as_bytes(), true, None)
}

#[test]
fn buffered_snapshot_failures_keep_safe_field_reasons_without_payload() {
    for (field, value, parameter) in [
        (
            "arguments",
            json!("synthetic-private-arguments"),
            "response.output_item.arguments",
        ),
        (
            "call_id",
            json!("synthetic-private-call"),
            "response.output_item.call_id",
        ),
        (
            "name",
            json!("synthetic-private-name"),
            "response.output_item.name",
        ),
        (
            "synthetic-private-key",
            json!("synthetic-private-value"),
            "response.output_item.changed",
        ),
    ] {
        let mut terminal = call();
        terminal[field] = value;
        let failure =
            buffered_wire(vec![done(Some(json!(0)), call())], json!([terminal])).unwrap_err();
        assert_eq!(failure.validation_parameter, Some(parameter));
        assert_eq!(failure.failure.category, crate::error_codes::STREAM_INVALID);
        assert_eq!(
            failure.execution.certainty,
            crate::scheduler::rotation::ExecutionCertainty::Unknown
        );
        let details = failure.upstream_error.unwrap();
        assert_eq!(details.parameter.as_deref(), Some(parameter));
        assert_eq!(details.sanitized().parameter.as_deref(), Some(parameter));
        assert!(
            details
                .message
                .as_ref()
                .unwrap()
                .contains("item_kind=function_call")
        );
        assert!(
            details
                .message
                .as_ref()
                .unwrap()
                .contains("snapshot_reason=field_")
        );
        assert!(
            !serde_json::to_string(&details)
                .unwrap()
                .contains("synthetic-private")
        );
        let message = failure.preserved.unwrap().message;
        assert!(message.contains(parameter));
        assert!(message.contains("item_kind=function_call"));
        assert!(message.contains("snapshot_reason=field_"));
        assert!(!message.contains("before client output"));
        assert!(!message.contains("synthetic-private"));
    }
}

#[test]
fn buffered_hosted_tool_diagnostics_survive_storage_without_action_contents() {
    let item = json!({"type":"web_search_call","id":"ws_synthetic","status":"completed",
        "action":{"type":"search","query":"synthetic-private-before"}});
    let mut terminal = item.clone();
    terminal["action"]["query"] = json!("synthetic-private-after");
    let failure = buffered_wire(vec![done(Some(json!(0)), item)], json!([terminal])).unwrap_err();
    assert_eq!(
        failure.validation_parameter,
        Some("response.output_item.action.query")
    );
    let details = failure.upstream_error.unwrap();
    let stored: crate::usage::UpstreamErrorDetails =
        serde_json::from_value(serde_json::to_value(details.sanitized()).unwrap()).unwrap();
    assert_eq!(
        stored.parameter.as_deref(),
        Some("response.output_item.action.query")
    );
    let message = stored.message.unwrap();
    assert!(message.contains("item_kind=web_search_call"));
    assert!(message.contains("snapshot_reason=field_changed"));
    assert!(message.contains("before=string, after=string"));
    assert!(!message.contains("synthetic-private"));
}

#[test]
fn buffered_web_search_optional_metadata_does_not_break_completion() {
    let item = json!({"type":"web_search_call","id":"ws_synthetic","status":"completed",
        "action":{"type":"search","query":"synthetic query"}});
    for action in [
        json!({"type":"search","query":"synthetic query","queries":null,"sources":null}),
        json!({"type":"search","query":"synthetic query","queries":["synthetic query"],
            "sources":[{"type":"url","url":"https://example.test/source"}]}),
    ] {
        let mut terminal = item.clone();
        terminal["action"] = action;
        let result = buffered_wire(
            vec![done(Some(json!(0)), item.clone())],
            json!([terminal.clone()]),
        );
        let bytes =
            result.unwrap_or_else(|_| panic!("compatible hosted-search metadata was rejected"));
        let response: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(response["output"][0], terminal);
    }
}

#[test]
fn buffered_invalid_shapes_and_sparse_indices_have_distinct_reasons() {
    for (events, parameter) in [
        (
            vec![done(Some(json!(-1)), call())],
            "response.output_item.index",
        ),
        (
            vec![done(Some(json!(1)), call())],
            "response.output_item.index",
        ),
    ] {
        let failure = buffered_wire(events, json!([])).unwrap_err();
        assert_eq!(failure.validation_parameter, Some(parameter));
        assert_eq!(
            failure.upstream_error.unwrap().parameter.as_deref(),
            Some(parameter)
        );
    }
    for (bytes, parameter) in [
        (b"[]".as_slice(), "response.json"),
        (
            b"data: {synthetic-private-invalid-json\n\n".as_slice(),
            "stream.event.json",
        ),
    ] {
        let failure = completed_upstream_response(bytes, true, None).unwrap_err();
        assert_eq!(failure.validation_parameter, Some(parameter));
        if parameter == "stream.event.json" {
            let details = failure.upstream_error.as_ref().unwrap();
            assert_eq!(details.error_type.as_deref(), Some("relay_stream_parser"));
            assert!(details.message.as_deref().unwrap().contains("line="));
        }
        assert!(
            !failure
                .upstream_error
                .unwrap()
                .message
                .unwrap()
                .contains("synthetic-private")
        );
    }
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
