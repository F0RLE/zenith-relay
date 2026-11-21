use super::*;

fn message() -> Value {
    json!({"type":"message","id":"msg_test","role":"assistant","status":"completed",
        "phase":"final_answer","content":[{"type":"output_text","text":"synthetic answer"}]})
}

fn without_status(mut item: Value) -> Value {
    item.as_object_mut().unwrap().remove("status");
    item
}

#[test]
fn an_omitted_completed_message_status_sends_only_the_new_turn() {
    let user = json!({"role":"user","content":"synthetic question"});
    let reasoning = json!({"type":"reasoning","id":"rs_test","summary":[],
        "encrypted_content":"opaque".repeat(1024)});
    let next = json!({"role":"user","content":"next"});
    let first = request(json!([user]));
    let replay = request(json!([user, reasoning, without_status(message()), next]));
    let parse = |body: &Value, debug| {
        planner::parse(&serde_json::to_vec(body).unwrap())
            .unwrap()
            .with_diagnostics(debug, None)
    };
    for capture in [false, true] {
        let saved = parse(&first, capture)
            .plan(None)
            .state
            .complete(&json!({"id":"first","status":"completed","output":[reasoning,message()]}))
            .unwrap();
        for debug in [false, true] {
            let planned = parse(&replay, debug).plan(Some(&saved));
            let wire: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(wire["previous_response_id"], "first");
            assert_eq!(wire["input"], json!([next]));
            assert!(planned.full_request_reason.is_none());
            assert_eq!(
                serde_json::to_value(planned.output_reconciled).unwrap(),
                if debug {
                    json!("omitted_completed_message_status")
                } else {
                    Value::Null
                }
            );
        }
    }
}

#[test]
fn message_status_omission_is_one_way_and_only_for_matching_completed_output() {
    let first = request(json!([]));
    let next = |item: &Value| request(json!([item, {"role":"user","content":"next"}]));
    let mut original = message();
    original["synthetic_extension"] = json!({"opaque":true});
    let bare = without_status(original.clone());
    let saved = checkpoint(&first, &json!([original]));
    let missing = checkpoint(&first, &json!([bare]));
    assert_full_history(&next(&original), &missing);
    for status in [Value::Null, json!("incomplete"), json!("in_progress")] {
        let mut other = bare.clone();
        other["status"] = status;
        assert_full_history(&next(&other), &saved);
        assert_full_history(&next(&bare), &checkpoint(&first, &json!([other])));
    }
    for field in ["id", "role", "phase", "content", "synthetic_extension"] {
        let mut edited = bare.clone();
        edited.as_object_mut().unwrap().remove(field);
        assert_full_history(&next(&edited), &saved);
    }
    let mut edited = bare.clone();
    edited["content"][0]["text"] = json!("different");
    assert_full_history(&next(&edited), &saved);
    let mut attributed = original.clone();
    attributed["agent"] = json!({"agent_name":"/root"});
    let with_agent = checkpoint(&first, &json!([attributed]));
    assert_eq!(
        plan(&next(&bare), Some(&with_agent)).output_reconciled,
        Some(crate::usage::SocketOutputReconciliation::OmittedAllowlistedOutputFields)
    );
    for (field, value) in [
        ("role", json!("user")),
        ("type", json!("unknown")),
        ("id", json!("")),
        ("content", Value::Null),
    ] {
        let mut malformed = original.clone();
        malformed[field] = value;
        let saved = checkpoint(&first, &json!([malformed]));
        assert_full_history(&next(&without_status(malformed)), &saved);
    }
    let prior = checkpoint(&request(json!([original])), &json!([message()]));
    let changed_history = request(json!([bare, message(), {"role":"user","content":"next"}]));
    assert_eq!(
        plan(&changed_history, Some(&prior)).full_request_reason,
        Some(FullRequestReason::SocketHistoryChanged)
    );
}
