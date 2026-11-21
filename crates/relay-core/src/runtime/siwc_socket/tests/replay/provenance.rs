use super::*;
use crate::usage::SocketOutputReconciliation;

fn output_items() -> Vec<Value> {
    vec![
        json!({"type":"reasoning","id":"rs_test","summary":[],
            "encrypted_content":"synthetic-cipher"}),
        json!({"type":"message","id":"msg_test","role":"assistant",
            "content":[{"type":"output_text","text":"synthetic answer"}]}),
        json!({"type":"function_call","id":"fc_test","call_id":"call_test",
            "name":"lookup","arguments":"{}","status":"completed"}),
        json!({"type":"custom_tool_call","id":"ct_test","call_id":"call_test",
            "name":"lookup","input":"synthetic input","status":"completed"}),
        json!({"type":"web_search_call","id":"ws_test","status":"completed",
            "action":{"type":"search","query":"synthetic lookup"}}),
    ]
}

fn append_turn(body: &Value, output: &Value) -> Value {
    let mut next = body.clone();
    next["input"].as_array_mut().unwrap().extend([
        output.clone(),
        json!({"role":"user","content":"synthetic next"}),
    ]);
    next
}

#[test]
fn only_an_optional_completed_status_may_be_omitted_from_a_function_call() {
    let first = request(json!([{"role":"user","content":"synthetic"}]));
    let mut call = output_items().remove(2);
    call["synthetic_extension"] = json!({"execution":"opaque"});
    let saved = checkpoint(&first, &json!([call]));
    let mut replay = call.clone();
    replay.as_object_mut().unwrap().remove("status");
    let next = append_turn(&first, &replay);
    let planned = planner::parse(&serde_json::to_vec(&next).unwrap())
        .unwrap()
        .plan(Some(&saved));
    let payload: Value = serde_json::from_str(&planned.payload).unwrap();
    assert_eq!(payload["previous_response_id"], "first");
    assert!(planned.output_reconciled.is_none()); // diagnostics off, same behavior

    for (field, value) in [
        ("status", json!("incomplete")),
        ("status", Value::Null),
        ("arguments", json!("{ }")),
        ("name", json!("changed")),
        ("id", json!("fc_changed")),
        ("call_id", json!("call_changed")),
        ("synthetic_extension", json!({"execution":"changed"})),
        ("added_extension", json!(true)),
    ] {
        let mut edited = replay.clone();
        edited[field] = value;
        assert_full_history(&append_turn(&first, &edited), &saved);
    }
    replay
        .as_object_mut()
        .unwrap()
        .remove("synthetic_extension");
    assert_full_history(&append_turn(&first, &replay), &saved);
}

#[test]
fn function_projection_allows_only_redundant_parsed_arguments() {
    let first = request(json!([{"role":"user","content":"synthetic"}]));
    let mut call = output_items().remove(2);
    call["arguments"] = json!("{\"flag\":true}");
    let next = append_turn(&first, &call);
    for decoration in [json!({"flag":true}), Value::Null, json!({"flag":false})] {
        let mut upstream = call.clone();
        upstream["parsed_arguments"] = decoration.clone();
        let saved = checkpoint(&first, &json!([upstream]));
        if decoration == json!({"flag":false}) {
            assert_full_history(&next, &saved);
        } else {
            let planned = plan(&next, Some(&saved));
            assert_eq!(planned.full_request_reason, None);
            assert_eq!(
                planned.output_reconciled,
                Some(SocketOutputReconciliation::OmittedParsedFunctionArguments)
            );
            let mut status_also_omitted = next.clone();
            status_also_omitted["input"][1]
                .as_object_mut()
                .unwrap()
                .remove("status");
            let planned = plan(&status_also_omitted, Some(&saved));
            assert_eq!(
                planned.output_reconciled,
                Some(SocketOutputReconciliation::OmittedOptionalFunctionFields)
            );
        }
        let mut changed = next.clone();
        changed["input"][1]["parsed_arguments"] = json!(["different parse"]);
        assert_full_history(&changed, &saved);
    }
}

#[test]
fn reasoning_created_by_omission_is_observed_only_on_a_proven_delta() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let mut output = output_items().remove(0);
    output["created_by"] = json!("synthetic-provenance");
    output["synthetic_extension"] = json!({"opaque":true});
    let saved = checkpoint(&body, &json!([output]));
    let mut replay = output.clone();
    replay.as_object_mut().unwrap().remove("created_by");
    let next = append_turn(&body, &replay);
    let planned = plan(&next, Some(&saved));
    assert_eq!(
        planned.output_reconciled,
        Some(SocketOutputReconciliation::OmittedReasoningCreatedBy)
    );
    let delta: Value = serde_json::from_str(&planned.payload).unwrap();
    assert_eq!(delta["previous_response_id"], "first");
    assert_eq!(delta["input"], json!([next["input"][2]]));
    assert!(
        plan(&append_turn(&body, &output), Some(&saved))
            .output_reconciled
            .is_none()
    );
    assert!(plan(&next, None).output_reconciled.is_none());

    for field in ["id", "summary", "encrypted_content"] {
        let mut edited = replay.clone();
        edited.as_object_mut().unwrap().remove(field);
        let next = append_turn(&body, &edited);
        assert_full_history(&next, &saved);
        assert!(plan(&next, Some(&saved)).output_reconciled.is_none());
    }
    replay
        .as_object_mut()
        .unwrap()
        .remove("synthetic_extension");
    let next = append_turn(&body, &replay);
    assert_full_history(&next, &saved);
    let planned = plan(&next, Some(&saved));
    assert!(planned.output_reconciled.is_none());
    let mismatch = planned.output_mismatch.unwrap();
    assert_eq!(
        mismatch.item_kind,
        Some(crate::usage::SocketReplayItemKind::Reasoning)
    );
    assert_eq!(
        mismatch.fields,
        vec![crate::usage::SocketReplayField::Other]
    );
    assert_eq!(
        mismatch.unknown_fields.unwrap(),
        crate::usage::SocketUnknownFieldChanges {
            removed: 1,
            single_removed_type: Some(crate::usage::SocketReplayValueKind::Object),
            ..Default::default()
        }
    );
    let observed = serde_json::to_string(&mismatch).unwrap();
    assert!(!observed.contains("synthetic_extension"));
    assert!(!observed.contains("synthetic-cipher"));
}

#[test]
fn mixed_reasoning_and_function_omissions_keep_original_order() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let items = output_items();
    let mut reasoning = items[0].clone();
    reasoning["created_by"] = json!("synthetic-provenance");
    let call = items[2].clone();
    let saved = checkpoint(&body, &json!([reasoning, call]));
    let mut replay_call = call;
    replay_call.as_object_mut().unwrap().remove("status");
    let mut next = append_turn(&body, &items[0]);
    next["input"].as_array_mut().unwrap().insert(2, replay_call);
    let planned = plan(&next, Some(&saved));
    assert_eq!(
        planned.output_reconciled,
        Some(SocketOutputReconciliation::OmittedAllowlistedOutputFields)
    );
    let delta: Value = serde_json::from_str(&planned.payload).unwrap();
    assert_eq!(delta["input"], json!([next["input"][3]]));
    next["input"].as_array_mut().unwrap().swap(1, 2);
    assert_full_history(&next, &saved);
}

#[test]
fn output_only_created_by_can_be_omitted_without_losing_continuation() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for item in output_items() {
        let mut attributed = item.clone();
        attributed["created_by"] = json!("synthetic-provider-provenance");
        for (output, replay) in [(&attributed, &item), (&item, &attributed)] {
            let checkpoint = checkpoint(&body, &json!([output]));
            let next = append_turn(&body, replay);
            let planned = plan(&next, Some(&checkpoint));
            assert_eq!(planned.full_request_reason, None, "{}", item["type"]);
            assert!(planned.output_mismatch.is_none());
            let omission = if item["type"] == "reasoning" {
                SocketOutputReconciliation::OmittedReasoningCreatedBy
            } else {
                SocketOutputReconciliation::OmittedAllowlistedOutputFields
            };
            assert_eq!(
                planned.output_reconciled,
                output.get("created_by").map(|_| omission)
            );
            let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "first");
            assert_eq!(encoded["input"], json!([next["input"][2]]));

            // Comparison normalization must not edit full requests or new input.
            let full: Value = serde_json::from_str(&plan(&next, None).payload).unwrap();
            assert_eq!(full["input"], next["input"]);
            let mut with_new_output = next.clone();
            with_new_output["input"]
                .as_array_mut()
                .unwrap()
                .push(attributed.clone());
            let delta: Value =
                serde_json::from_str(&plan(&with_new_output, Some(&checkpoint)).payload).unwrap();
            assert_eq!(delta["input"][1], attributed);

            // A second checkpoint still compares the complete normalized prefix.
            let second_output = json!({"type":"message","id":"msg_second",
                "role":"assistant","content":[],"created_by":"synthetic-provenance"});
            let second = planned
                .state
                .complete(&json!({"id":"second",
                "status":"completed","output":[second_output]}))
                .unwrap();
            let mut next_again = next.clone();
            next_again["input"][1] = item.clone();
            let mut replay_second = second_output;
            replay_second.as_object_mut().unwrap().remove("created_by");
            next_again = append_turn(&next_again, &replay_second);
            let delta: Value =
                serde_json::from_str(&plan(&next_again, Some(&second)).payload).unwrap();
            assert_eq!(delta["previous_response_id"], "second");
            assert_eq!(delta["input"].as_array().unwrap().len(), 1);
        }
    }
}

#[test]
fn unknown_non_null_omission_does_not_borrow_the_provenance_exception() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for item in output_items().into_iter().take(3) {
        let mut output = item.clone();
        output["created_by"] = json!("synthetic-provenance");
        output["synthetic-extension"] = json!("synthetic-opaque-value");
        let checkpoint = checkpoint(&body, &json!([output]));
        let next = append_turn(&body, &item);
        let planned = plan(&next, Some(&checkpoint));
        assert_eq!(
            planned.full_request_reason,
            Some(FullRequestReason::SocketOutputChanged)
        );
        assert!(planned.output_reconciled.is_none());
        let mismatch = planned.output_mismatch.unwrap();
        assert_eq!(mismatch.fields, [crate::usage::SocketReplayField::Other]);
        assert_eq!(
            mismatch.unknown_fields.unwrap(),
            crate::usage::SocketUnknownFieldChanges {
                removed: 1,
                single_removed_type: Some(crate::usage::SocketReplayValueKind::String),
                ..Default::default()
            }
        );
        let full: Value = serde_json::from_str(&planned.payload).unwrap();
        assert_eq!(full["input"], next["input"]);
        assert!(full.get("previous_response_id").is_none());
    }
}

#[test]
fn provenance_normalization_keeps_content_identity_and_extensions_strict() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for mut output in output_items() {
        output["created_by"] = json!("synthetic-provenance");
        output["synthetic_extension"] = json!("synthetic-private-value");
        let checkpoint = checkpoint(&body, &json!([output]));
        let mut replay = output.clone();
        replay.as_object_mut().unwrap().remove("created_by");
        for field in ["id", "synthetic_extension"] {
            let mut changed = replay.clone();
            changed.as_object_mut().unwrap().remove(field);
            assert_full_history(&append_turn(&body, &changed), &checkpoint);
        }
        for field in [
            "encrypted_content",
            "content",
            "arguments",
            "input",
            "call_id",
            "action",
        ] {
            if replay.get(field).is_some() {
                let mut changed = replay.clone();
                changed[field] = json!("synthetic-changed");
                assert_full_history(&append_turn(&body, &changed), &checkpoint);
            }
        }
        for invalid in [Value::Null, json!(false), json!(1), json!([]), json!({})] {
            let mut changed = replay.clone();
            changed["created_by"] = invalid;
            assert_full_history(&append_turn(&body, &changed), &checkpoint);
        }
    }
    for output in [
        json!({"type":"message","role":"user","content":"synthetic"}),
        json!({"type":"synthetic_extension","id":"synthetic"}),
        json!({"role":"assistant","content":"synthetic"}),
    ] {
        let mut attributed = output.clone();
        attributed["created_by"] = json!("synthetic-provenance");
        let checkpoint = checkpoint(&body, &json!([attributed]));
        assert_full_history(&append_turn(&body, &output), &checkpoint);
    }
}

#[test]
fn output_attribution_omission_is_observed_only_on_a_proven_delta() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let field = "internal_chat_message_metadata_passthrough";
    for output in output_items() {
        let mut attributed = output.clone();
        attributed[field] = json!({"turn_id":"synthetic", "content_token_count":20});
        let saved = checkpoint(&body, &json!([attributed]));
        let next = append_turn(&body, &output);
        let planned = plan(&next, Some(&saved));
        assert_eq!(planned.full_request_reason, None);
        assert_eq!(
            planned.output_reconciled,
            Some(SocketOutputReconciliation::OmittedAllowlistedOutputFields)
        );
        let delta: Value = serde_json::from_str(&planned.payload).unwrap();
        assert_eq!(delta["input"], json!([next["input"][2]]));
        assert!(
            plan(&append_turn(&body, &attributed), Some(&saved))
                .output_reconciled
                .is_none()
        );

        attributed["synthetic_extension"] = json!({"opaque":true});
        let saved = checkpoint(&body, &json!([attributed]));
        let planned = plan(&next, Some(&saved));
        assert_eq!(
            planned.full_request_reason,
            Some(FullRequestReason::SocketOutputChanged)
        );
        assert!(planned.output_reconciled.is_none());
    }
}

#[test]
fn web_search_attribution_does_not_break_the_next_user_turn() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let mut output = json!({"type":"web_search_call","id":"ws_test","status":"completed",
        "action":{"type":"search","query":"synthetic"},
        "internal_chat_message_metadata_passthrough":{"turn_id":"synthetic","content_token_count":20}});
    let previous = checkpoint(&body, &json!([output]));
    let field = "internal_chat_message_metadata_passthrough";
    output.as_object_mut().unwrap().remove(field);
    let next = append_turn(&body, &output);
    let delta: Value = serde_json::from_str(&plan(&next, Some(&previous)).payload).unwrap();
    assert_eq!(delta["previous_response_id"], "first");
    assert_eq!(delta["input"], json!([next["input"][2]]));

    let mut edited = output.clone();
    edited["action"]["query"] = json!("synthetic changed");
    assert_full_history(&append_turn(&body, &edited), &previous);

    for metadata in [
        json!({"executed_tool_calls":[{"name":"synthetic"}]}),
        json!({"synthetic_extension":{}}),
    ] {
        let mut attributed = output.clone();
        attributed[field] = metadata;
        let previous = checkpoint(&body, &json!([attributed]));
        assert_full_history(&next, &previous);
    }
}
