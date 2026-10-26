use super::*;

#[test]
fn native_replay_shared_snapshot_preserves_history_and_byte_limits() {
    let mut request = json!({"input":[{"role":"user","content":"Synthetic original history"}]});
    let mut response = json!({"id":"synthetic-response","output":[{
        "type":"function_call","call_id":"synthetic-call","name":"lookup","arguments":"{}"
    }]});
    let expected = json!({
        "model":"synthetic-model", "initial_input":request["input"],
        "completed_output":response["output"]
    });
    let (_, state) =
        NativeResponsesReplayState::from_response(&request, "synthetic-model", &response).unwrap();
    assert_eq!(serde_json::to_value(&state).unwrap(), expected);
    let bytes = serde_json::to_vec(&expected).unwrap().len();
    let mut store = NativeResponsesReplayStore::with_limits(1, 100, bytes, bytes);
    store.insert("key", "synthetic-response", "route", state.clone(), 10);
    assert_eq!(store.memory_usage(), (1, bytes, bytes));
    let held = store.get("key", "synthetic-response", "route", 10).unwrap();
    let another = store.get("key", "synthetic-response", "route", 10).unwrap();
    assert_eq!(store.memory_usage(), (1, bytes, bytes));
    assert!(store
        .get("other-key", "synthetic-response", "route", 10)
        .is_none());
    assert!(store
        .get("key", "synthetic-response", "other-route", 10)
        .is_none());
    assert!(store
        .get("key", "synthetic-response", "route", 110)
        .is_some());
    assert!(store
        .get("key", "synthetic-response", "route", 111)
        .is_none());
    assert_eq!(store.memory_usage(), (0, 0, bytes));

    // Callers could already hold an owned snapshot after expiry. Sharing must
    // neither extend cache admission nor expose mutable request/output state.
    request["input"][0]["content"] = json!("Changed source input");
    response["output"][0]["arguments"] = json!("Changed source output");
    let continuation = json!({
        "previous_response_id":"synthetic-response",
        "input":[{"type":"function_call_output","call_id":"synthetic-call","output":"ok"}],
        "instructions":"Current instructions", "tools":[], "tool_choice":"none"
    });
    let replayed = held
        .replay_request(&continuation, "synthetic-model", true)
        .unwrap();
    assert_eq!(
        replayed["input"],
        json!([
            expected["initial_input"][0],
            expected["completed_output"][0],
            continuation["input"][0]
        ])
    );
    assert_eq!(replayed["instructions"], continuation["instructions"]);
    assert_eq!(replayed["tools"], continuation["tools"]);
    assert_eq!(replayed["tool_choice"], continuation["tool_choice"]);
    assert!(replayed.get("previous_response_id").is_none());
    let mut changed = replayed.clone();
    changed["input"][0]["content"] = json!("Changed outgoing input");
    changed["input"][1]["arguments"] = json!("Changed outgoing output");
    assert_eq!(
        another
            .replay_request(&continuation, "synthetic-model", true)
            .unwrap(),
        replayed
    );
    assert!(held
        .replay_request(&continuation, "other-model", true)
        .is_err());

    let mut too_small = NativeResponsesReplayStore::with_limits(1, 100, bytes - 1, bytes);
    too_small.insert("key", "synthetic-response", "route", state, 10);
    assert_eq!(too_small.memory_usage(), (0, 0, bytes));
    assert!(too_small
        .get("key", "synthetic-response", "route", 10)
        .is_none());
}

#[test]
fn native_replay_string_predecessor_requires_exact_materialized_message() {
    let snapshot = |input: Value| {
        NativeResponsesReplayState::from_response(
            &json!({"input":input}),
            "synthetic-model",
            &json!({"id":"synthetic-response","output":[]}),
        )
        .unwrap()
        .1
    };
    for text in ["", "Synthetic text", "Synthetic Unicode: \u{1f980}"] {
        let previous = snapshot(json!(text));
        let canonical = json!({"role":"user","content":[{"type":"input_text","text":text}]});
        assert!(previous.is_materialized_predecessor_of(&snapshot(json!([canonical]))));
        let mut mismatches = Vec::new();
        let mut changed = canonical.clone();
        changed["type"] = json!("message");
        mismatches.push(changed);
        let mut changed = canonical.clone();
        changed["role"] = json!("assistant");
        mismatches.push(changed);
        let mut changed = canonical.clone();
        changed["content"][0]["extra"] = json!(true);
        mismatches.push(changed);
        let mut changed = canonical.clone();
        changed["content"][0]["type"] = json!("output_text");
        mismatches.push(changed);
        let mut changed = canonical.clone();
        changed["content"][0]["text"] = json!(format!("{text} changed"));
        mismatches.push(changed);
        let mut changed = canonical.clone();
        changed["content"]
            .as_array_mut()
            .unwrap()
            .push(canonical["content"][0].clone());
        mismatches.push(changed);
        mismatches.push(json!({"role":"user","content":text}));
        for changed in mismatches {
            assert!(!previous.is_materialized_predecessor_of(&snapshot(json!([changed]))));
        }
    }
}
