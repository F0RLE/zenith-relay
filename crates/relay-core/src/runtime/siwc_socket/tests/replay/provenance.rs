use super::*;

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
