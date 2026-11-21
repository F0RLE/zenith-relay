use super::*;
use crate::usage::{SocketOutputReconciliation, SocketReplayField};

const TRACE: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";
const NATIVE: &str = "internal_chat_message_metadata_passthrough";

fn traced(item: &Value) -> Value {
    let mut original = item.clone();
    original["metadata"] = json!({"turn_id":TRACE});
    original[NATIVE] = json!({"turn_id":TRACE, "content_token_count":3});
    original
}

fn next(item: &Value) -> Value {
    request(json!([item, {"role":"user","content":"next"}]))
}

#[test]
fn only_a_duplicate_turn_id_can_be_omitted_from_matching_completed_items() {
    for item in [
        json!({"type":"reasoning","id":"rs_test","encrypted_content":"synthetic","summary":[]}),
        json!({"type":"function_call","id":"fc_test","call_id":"call_test","name":"lookup","arguments":"{}"}),
        json!({"type":"custom_tool_call","id":"ct_test","call_id":"call_test","name":"edit","input":"test"}),
        json!({"type":"message","role":"assistant","id":"msg_test","content":[]}),
        json!({"type":"web_search_call","id":"ws_test","status":"completed",
            "action":{"type":"search","query":"synthetic"}}),
    ] {
        let original = traced(&item);
        let first = request(json!([]));
        for capture in [false, true] {
            let saved = planner::parse(&serde_json::to_vec(&first).unwrap())
                .unwrap()
                .with_diagnostics(capture, None)
                .plan(None)
                .state
                .complete(&json!({"status":"completed","id":"first","output":[original]}))
                .unwrap();
            for diagnostics in [false, true] {
                let body = next(&item);
                let planned = planner::parse(&serde_json::to_vec(&body).unwrap())
                    .unwrap()
                    .with_diagnostics(diagnostics, None)
                    .plan(Some(&saved));
                let wire: Value = serde_json::from_str(&planned.payload).unwrap();
                assert_eq!(wire["input"], json!([body["input"][1]]));
                assert_eq!(wire["previous_response_id"], "first");
                assert_eq!(
                    planned.output_reconciled,
                    diagnostics.then_some(SocketOutputReconciliation::OmittedDuplicateTurnMetadata)
                );
            }
        }
        let saved = checkpoint(&first, &json!([original]));
        assert!(
            plan(&next(&original), Some(&saved))
                .output_reconciled
                .is_none()
        );
        assert_full_history(&next(&original), &checkpoint(&first, &json!([item])));
        for field in [
            "id",
            "encrypted_content",
            "summary",
            "call_id",
            "name",
            "arguments",
            "input",
            "content",
            "action",
        ] {
            if item.get(field).is_some() {
                let mut edited = item.clone();
                edited.as_object_mut().unwrap().remove(field);
                assert_full_history(&next(&edited), &saved);
            }
        }
    }
}

#[test]
fn changed_metadata_missing_witness_and_edits_to_older_input_require_full_history() {
    let item = json!({"type":"reasoning","id":"rs_test","summary":[]});
    let first = request(json!([]));
    let original = traced(&item);
    let saved = checkpoint(&first, &json!([original]));
    for metadata in [
        Value::Null,
        json!({}),
        json!({"turn_id":OTHER}),
        json!({"turn_id":TRACE,"extra":null}),
        json!({"turn_id":"not-a-uuid"}),
        json!({"turn_id":7}),
    ] {
        let mut edited = original.clone();
        edited["metadata"] = metadata;
        assert_full_history(&next(&edited), &saved);
        assert_full_history(&next(&item), &checkpoint(&first, &json!([edited])));
        let mismatch = plan(&next(&edited), Some(&saved)).output_mismatch.unwrap();
        assert!(mismatch.fields.contains(&SocketReplayField::ItemMetadata));
        assert!(!serde_json::to_string(&mismatch).unwrap().contains(TRACE));
    }
    for witness in [json!({"turn_id":OTHER}), json!({"turn_id":null}), json!({})] {
        let mut unproven = original.clone();
        unproven[NATIVE] = witness;
        assert_full_history(&next(&item), &checkpoint(&first, &json!([unproven])));
    }
    let mut altered = item.clone();
    altered[NATIVE] = json!({"turn_id":OTHER});
    assert_full_history(&next(&altered), &saved);
    for unsupported in [
        json!({"type":"reasoning","summary":[]}),
        json!({"type":"message","role":"user","id":"msg_test","content":[]}),
        json!({"type":"function_call","id":"fc_test","name":"lookup","arguments":"{}"}),
        json!({"type":"function_call_output","id":"fc_test","call_id":"test","output":"ok"}),
        json!({"type":"web_search_call","status":"completed","action":{"type":"search"}}),
        json!({"type":"web_search_call","id":"ws_test","status":"in_progress"}),
    ] {
        assert_full_history(
            &next(&unsupported),
            &checkpoint(&first, &json!([traced(&unsupported)])),
        );
    }
    let historical = checkpoint(&request(json!([original])), &json!([item]));
    let body = request(json!([item, item, {"role":"user","content":"next"}]));
    assert_eq!(
        plan(&body, Some(&historical)).full_request_reason,
        Some(FullRequestReason::SocketHistoryChanged)
    );
}

#[test]
fn web_search_replay_keeps_its_action_status_and_metadata_proof() {
    let item = json!({"type":"web_search_call","id":"ws_test","status":"completed",
        "action":{"type":"search","query":"synthetic"}});
    let first = request(json!([]));
    let original = traced(&item);
    let saved = checkpoint(&first, &json!([original]));
    let mut replay = original.clone();
    replay.as_object_mut().unwrap().remove("metadata");
    assert_eq!(
        plan(&next(&replay), Some(&saved)).output_reconciled,
        Some(SocketOutputReconciliation::OmittedDuplicateTurnMetadata)
    );
    for (field, replacement) in [
        ("status", json!("failed")),
        ("action", json!({"type":"search","query":"different"})),
        ("agent", json!({"agent_name":"/root"})),
        ("result", json!("different")),
        ("metadata", json!({"turn_id":OTHER})),
        ("metadata", json!({"turn_id":TRACE,"example":true})),
    ] {
        let mut edited = replay.clone();
        edited[field] = replacement;
        assert_full_history(&next(&edited), &saved);
    }
    let mut attributed = original.clone();
    attributed["agent"] = json!({"agent_name":"/root"});
    assert_full_history(&next(&replay), &checkpoint(&first, &json!([attributed])));
    let mut unproven = original;
    unproven[NATIVE]["turn_id"] = json!(OTHER);
    assert_full_history(&next(&item), &checkpoint(&first, &json!([unproven])));
    let historical = checkpoint(&request(json!([traced(&item)])), &json!([item]));
    let replay = request(json!([item, item, {"role":"user","content":"next"}]));
    assert_eq!(
        plan(&replay, Some(&historical)).full_request_reason,
        Some(FullRequestReason::SocketHistoryChanged)
    );
}
