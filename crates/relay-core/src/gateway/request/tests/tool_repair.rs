use super::*;
use serde_json::json;

#[test]
fn stale_custom_tool_recovery_removes_only_the_reported_historical_call() {
    let error = br#"{"error":{"message":"No tool output found for custom tool call ctc_stale."}}"#;
    for call in [
        json!({"type":"custom_tool_call","id":"item_stale","call_id":"ctc_stale","name":"patch","input":"{}"}),
        json!({"type":"custom_tool_call","id":"ctc_stale","name":"patch","input":"{}"}),
    ] {
        let mut request = json!({"input":[
            {"type":"message","role":"user","content":"start"},
            {"type":"function_call","call_id":"fc_done","name":"lookup","arguments":"{}"},
            {"type":"function_call_output","call_id":"fc_done","output":"keep this result"},
            call,
            {"type":"message","role":"user","content":"continue"}
        ]});

        assert!(remove_unpaired_responses_tool_call(&mut request, 4, error));
        assert_eq!(request["input"].as_array().unwrap().len(), 4);
        assert_eq!(request["input"][0]["content"], "start");
        assert_eq!(request["input"][1]["call_id"], "fc_done");
        assert_eq!(request["input"][2]["output"], "keep this result");
        assert_eq!(request["input"][3]["content"], "continue");
    }
}
#[test]
fn stale_tool_recovery_leaves_ambiguous_mismatched_and_current_calls_untouched() {
    let error = br#"{"error":{"message":"No tool output found for custom tool call ctc_stale."}}"#;
    for (input, historical_item_count) in [
        (
            json!([
                {"type":"custom_tool_call","call_id":"ctc_stale","name":"patch","input":"{}"},
                {"type":"custom_tool_call","call_id":"ctc_other","name":"patch","input":"{}"}
            ]),
            2,
        ),
        (
            json!([
                {"type":"custom_tool_call","call_id":"ctc_other","name":"patch","input":"{}"}
            ]),
            1,
        ),
        (
            json!([
                {"type":"message","role":"user","content":"current"},
                {"type":"custom_tool_call","call_id":"ctc_stale","name":"patch","input":"{}"}
            ]),
            1,
        ),
    ] {
        let mut request = json!({"input":input});
        let original = request.clone();
        assert!(!remove_unpaired_responses_tool_call(
            &mut request,
            historical_item_count,
            error,
        ));
        assert_eq!(request, original);
    }
}
#[test]
fn tool_affinity_extracts_all_bounded_output_ids() {
    let request = json!({
        "previous_response_id": "resp_old",
        "input": [
            {"type": "custom_tool_call_output", "call_id": "ctc_old"},
            {"type": "custom_tool_call_output", "call_id": "ctc_old"},
            {"type": "function_call_output", "call_id": "function"},
            {"type": "custom_tool_call_output", "call_id": "   "}
        ]
    });
    assert_eq!(tool_call_output_ids(&request), vec!["ctc_old", "function"]);

    let response = json!({
        "id": "resp_old",
        "output": [{
            "type": "custom_tool_call",
            "id": "ctc_item",
            "call_id": "call_custom",
            "input": "Get-ChildItem"
        }]
    });
    for envelope in [
        response.clone(),
        json!({"type":"response.completed", "response":response}),
        json!({"type":"response.output_item.done", "item":response["output"][0]}),
    ] {
        assert_eq!(
            response_tool_call_ids(&envelope),
            vec!["call_custom", "ctc_item"]
        );
    }

    let paired = json!({
        "input": [
            {"type": "computer_call", "id": "computer_item", "call_id": "computer_call"},
            {"type": "computer_call_output", "call_id": "computer_call"}
        ]
    });
    assert_eq!(
        response_tool_call_ids(&paired),
        vec!["computer_call", "computer_item"]
    );
    assert_eq!(tool_call_output_ids(&paired), vec!["computer_call"]);
}
#[test]
fn legacy_responses_call_id_repair_preserves_items_and_tool_namespaces() {
    let mut request = json!({
        "input": [
            {"type": "message", "role": "user", "content": "continue"},
            {"type": "function_call", "id": "fc_existing", "name": "lookup", "namespace": "functions", "arguments": "{}"},
            {"type": "function_call_output", "name": "lookup", "output": "lookup result"},
            {"type": "custom_tool_call", "name": "patch", "namespace": "tools", "input": "{}"},
            {"type": "custom_tool_call_output", "name": "patch", "output": "patch result"},
            {"type": "function_call_output", "name": "heartbeat", "output": "keep standalone"}
        ]
    });

    assert!(repair_legacy_responses_call_ids(&mut request));
    let input = request["input"].as_array().expect("input array");
    assert_eq!(input.len(), 6);
    assert_eq!(input[1]["type"], "function_call");
    assert_eq!(input[1]["call_id"], "fc_existing");
    assert_eq!(input[1]["id"], "fc_existing");
    assert_eq!(input[1]["namespace"], "functions");
    assert_eq!(input[2]["call_id"], input[1]["call_id"]);
    assert!(input[3]["call_id"]
        .as_str()
        .is_some_and(|id| id.starts_with("call_missing_")));
    assert_eq!(input[3]["namespace"], "tools");
    assert_eq!(input[4]["call_id"], input[3]["call_id"]);
    assert_eq!(input[5]["name"], "heartbeat");
    assert!(input[5].get("call_id").is_none());
}
#[test]
fn tool_link_repair_keeps_explicit_ids_and_resolves_item_id_references() {
    for kind in ["function_call", "custom_tool_call"] {
        for call_id in [None, Some("call_stable")] {
            let mut call = json!({"type":kind,"id":"item_legacy","name":"lookup"});
            if let Some(id) = call_id {
                call["call_id"] = json!(id);
            }
            let mut request = json!({"input":[
                call,
                {"type":format!("{kind}_output"),"call_id":"item_legacy","output":"synthetic result"}
            ]});
            assert!(repair_legacy_responses_call_ids(&mut request));
            let expected = call_id.unwrap_or("item_legacy");
            assert_eq!(request["input"][0]["call_id"], expected);
            assert_eq!(request["input"][1]["call_id"], expected);
            assert_eq!(request["input"][0]["id"], "item_legacy");
            assert_eq!(request["input"][1]["output"], "synthetic result");
            assert!(!repair_legacy_responses_call_ids(&mut request));
        }
    }
}
#[test]
fn tool_link_repair_is_atomic_for_orphans_ambiguous_and_mismatched_results() {
    let call = json!({"type":"function_call","name":"lookup","arguments":"{}"});
    for invalid in [
        json!([{ "type":"function_call_output","output":"orphan" }]),
        json!([call, {"type":"custom_tool_call_output","output":"wrong kind"}]),
        json!([call, {"type":"function_call_output","name":"different","output":"wrong name"}]),
        json!([call, {"type":"function_call_output","namespace":"different","output":"wrong namespace"}]),
        json!([call, call, {"type":"function_call_output","output":"ambiguous"}]),
        json!([{"type":"function_call","id":"alias","call_id":"canonical"},
                {"type":"function_call","id":"other","call_id":"alias"},
                {"type":"function_call_output","call_id":"alias","output":"ambiguous alias"}]),
    ] {
        let mut input = vec![
            call.clone(),
            json!({"type":"function_call_output","output":"paired"}),
        ];
        input.extend(invalid.as_array().unwrap().iter().cloned());
        let mut request = json!({"input": input});
        let original = request.clone();
        assert!(!repair_legacy_responses_call_ids(&mut request));
        assert_eq!(request, original);
    }
}
#[test]
fn tool_link_repair_matches_parallel_results_by_namespace_without_reordering() {
    let mut request = json!({"input":[
        {"type":"function_call","name":"lookup","namespace":"first","arguments":"{}"},
        {"type":"function_call","name":"lookup","namespace":"second","arguments":"{}"},
        {"type":"function_call_output","name":"lookup","namespace":"second","output":"second result"},
        {"type":"function_call_output","name":"lookup","namespace":"first","output":"first result"}
    ]});
    assert!(repair_legacy_responses_call_ids(&mut request));
    assert_eq!(
        request["input"][0]["call_id"],
        request["input"][3]["call_id"]
    );
    assert_eq!(
        request["input"][1]["call_id"],
        request["input"][2]["call_id"]
    );
    assert_ne!(
        request["input"][0]["call_id"],
        request["input"][1]["call_id"]
    );
    assert_eq!(request["input"][2]["output"], "second result");
}
#[test]
fn legacy_responses_call_id_repair_is_idempotent_and_preserves_valid_history() {
    let mut request = json!({
        "input": [
            {"type": "function_call", "call_id": "known", "name": "lookup", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "known", "output": "ok"},
            {"type": "custom_tool_call", "call_id": "custom", "name": "patch", "input": "{}"},
            {"type": "custom_tool_call_output", "call_id": "custom", "output": "done"}
        ]
    });
    let original = request.clone();

    assert!(!repair_legacy_responses_call_ids(&mut request));
    assert_eq!(request, original);

    let mut legacy = json!({
        "input": [
            {"type": "function_call", "name": "lookup", "arguments": "{}"},
            {"type": "function_call_output", "output": "ok"}
        ]
    });
    assert!(repair_legacy_responses_call_ids(&mut legacy));
    let repaired = legacy.clone();
    assert!(!repair_legacy_responses_call_ids(&mut legacy));
    assert_eq!(legacy, repaired);
}
#[test]
fn legacy_responses_call_id_repair_fails_closed_at_history_bounds() {
    let mut too_many_items =
        json!({"input": vec![json!({"role": "user"}); MAX_LEGACY_RESPONSES_REPAIR_ITEMS + 1]});
    let original = too_many_items.clone();
    assert!(!repair_legacy_responses_call_ids(&mut too_many_items));
    assert_eq!(too_many_items, original);

    let mut too_many_calls = json!({
        "input": (0..=MAX_LEGACY_RESPONSES_PENDING_CALLS)
            .map(|index| json!({"type": "function_call", "name": format!("tool-{index}")}))
            .collect::<Vec<_>>()
    });
    let original = too_many_calls.clone();
    assert!(!repair_legacy_responses_call_ids(&mut too_many_calls));
    assert_eq!(too_many_calls, original);
}
