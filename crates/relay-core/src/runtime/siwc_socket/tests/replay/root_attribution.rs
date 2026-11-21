use super::*;
use crate::usage::{SocketOutputReconciliation, SocketReplayField};

fn items() -> [Value; 4] {
    [
        json!({"type":"reasoning","id":"rs_test","summary":[],"encrypted_content":"synthetic cipher"}),
        json!({"type":"function_call","id":"fc_test","call_id":"call_test","name":"lookup",
            "arguments":"{}","status":"completed"}),
        json!({"type":"custom_tool_call","id":"ct_test","call_id":"custom_test","name":"edit","input":"synthetic edit"}),
        json!({"type":"message","id":"msg_test","role":"assistant","content":[{"type":"output_text","text":"synthetic"}]}),
    ]
}

fn root() -> Value {
    json!({"agent_name":"/root"})
}

fn next(replay: &Value) -> Value {
    request(json!([replay, {"role":"user","content":"synthetic follow-up"}]))
}

#[test]
fn only_the_omitted_root_author_can_refer_to_original_completed_output() {
    for item in items() {
        let mut original = item.clone();
        original["agent"] = root();
        let bare_body = request(json!([]));
        let without_diagnostics = |body: &Value, saved: Option<&planner::Checkpoint>| {
            planner::parse(&serde_json::to_vec(body).unwrap())
                .unwrap()
                .plan(saved)
        };
        let no_debug = without_diagnostics(&bare_body, None)
            .state
            .complete(&json!({"id":"first","status":"completed","output":[original]}))
            .unwrap();
        let normal = without_diagnostics(&next(&item), Some(&no_debug));
        assert!(normal.output_reconciled.is_none() && normal.full_request_reason.is_none());
        let saved = checkpoint(&request(json!([])), &json!([original]));
        let exact = plan(&next(&original), Some(&saved));
        assert!(exact.full_request_reason.is_none() && exact.output_reconciled.is_none());
        let omitted = plan(&next(&item), Some(&saved));
        assert_eq!(
            omitted.output_reconciled,
            Some(SocketOutputReconciliation::OmittedRootAgentAttribution)
        );
        let delta: Value = serde_json::from_str(&omitted.payload).unwrap();
        assert_eq!(delta["previous_response_id"], "first");
        assert_eq!(delta["input"], json!([next(&item)["input"][1]]));

        let bare = checkpoint(&request(json!([])), &json!([item]));
        assert_full_history(&next(&original), &bare);
        for invalid in [
            Value::Null,
            json!("/root"),
            json!({}),
            json!({"agent_name":"/root/worker"}),
            json!({"agent_name":"/root","extra":"synthetic"}),
        ] {
            let mut other = item.clone();
            other["agent"] = invalid;
            assert_full_history(&next(&other), &saved);
            let other_saved = checkpoint(&request(json!([])), &json!([other]));
            assert_full_history(&next(&item), &other_saved);
            let mismatch = plan(&next(&item), Some(&other_saved))
                .output_mismatch
                .unwrap();
            assert_eq!(mismatch.fields, [SocketReplayField::Agent]);
            assert!(!serde_json::to_string(&mismatch).unwrap().contains("worker"));
        }
        for field in [
            "id",
            "summary",
            "encrypted_content",
            "call_id",
            "arguments",
            "input",
            "role",
            "content",
        ] {
            if item.get(field).is_some() {
                let mut changed = item.clone();
                changed.as_object_mut().unwrap().remove(field);
                assert_full_history(&next(&changed), &saved);
            }
        }
        let mut conflicting = item;
        conflicting["created_by"] = json!("/root/worker");
        assert_full_history(&next(&conflicting), &saved);
    }
}

#[test]
fn root_omission_is_not_available_for_other_roles_calls_results_or_edited_prior_input() {
    for item in [
        json!({"type":"message","id":"msg_test","role":"user","content":[]}),
        json!({"type":"reasoning","summary":[]}),
        json!({"type":"function_call","id":"fc_test","arguments":"{}","name":"lookup"}),
        json!({"type":"function_call_output","id":"out_test","call_id":"call_test","output":"ok"}),
        json!({"type":"agent_message","id":"msg_test","author":"/root","recipient":"/root/worker","content":[]}),
        json!({"type":"reasoning","id":"rs_test","summary":[],"created_by":"/root/worker"}),
    ] {
        let mut attributed = item.clone();
        attributed["agent"] = root();
        let saved = checkpoint(&request(json!([])), &json!([attributed]));
        assert_full_history(&next(&item), &saved);
    }
    let [mut historical, call, ..] = items();
    historical["agent"] = root();
    let saved = checkpoint(&request(json!([historical])), &json!([call]));
    historical.as_object_mut().unwrap().remove("agent");
    let changed = request(json!([historical, call, {"role":"user","content":"next"}]));
    assert_eq!(
        plan(&changed, Some(&saved)).full_request_reason,
        Some(FullRequestReason::SocketHistoryChanged)
    );
}
