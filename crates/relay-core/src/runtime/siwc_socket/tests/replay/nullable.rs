use super::*;

#[test]
fn optional_codex_fields_compare_null_and_absent_without_rewriting_wire_history() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for (item, field, populated) in [
        (
            json!({"type":"function_call","id":"fc_test","call_id":"call_test",
                "name":"lookup","arguments":"{}","status":"completed"}),
            "encrypted_function_args",
            json!(["synthetic-opaque-arguments"]),
        ),
        (
            json!({"type":"function_call","id":"fc_test","call_id":"call_test",
                "name":"lookup","arguments":"{}","status":"completed"}),
            "namespace",
            json!("synthetic_namespace"),
        ),
        (
            json!({"type":"custom_tool_call","id":"ct_test","call_id":"call_test",
                "name":"edit","input":"synthetic","status":"completed"}),
            "namespace",
            json!("synthetic_namespace"),
        ),
        (
            json!({"type":"function_call","id":"fc_test","call_id":"call_test",
                "name":"lookup","arguments":"{}","status":"completed"}),
            "async",
            json!(true),
        ),
        (
            json!({"type":"custom_tool_call","id":"ct_test","call_id":"call_test",
                "name":"edit","input":"synthetic","status":"completed"}),
            "async",
            json!(false),
        ),
        (
            json!({"type":"function_call","id":"fc_test","call_id":"call_test",
                "name":"lookup","arguments":"{}","status":"completed"}),
            "encrypted_content",
            json!("synthetic-opaque-content"),
        ),
        (
            json!({"type":"custom_tool_call","id":"ct_test","call_id":"call_test",
                "name":"edit","input":"synthetic","status":"completed"}),
            "encrypted_content",
            json!("synthetic-opaque-content"),
        ),
        (
            json!({"type":"message","id":"msg_test","role":"assistant",
                "status":"completed","content":[{"type":"output_text","text":"synthetic"}]}),
            "encrypted_content",
            json!("synthetic-opaque-content"),
        ),
    ] {
        let mut null = item.clone();
        null[field] = Value::Null;
        for (output, replay) in [(&item, &null), (&null, &item)] {
            let checkpoint = checkpoint(&body, &json!([output]));
            let suffix = json!({"role":"user","content":"synthetic next"});
            let mut next = body.clone();
            next["input"]
                .as_array_mut()
                .unwrap()
                .extend([replay.clone(), suffix.clone()]);
            let planned = plan(&next, Some(&checkpoint));
            assert_eq!(planned.full_request_reason, None, "{field}");
            let encoded: Value = serde_json::from_str(&planned.payload).unwrap();
            assert_eq!(encoded["previous_response_id"], "first");
            assert_eq!(encoded["input"], json!([suffix]));
            let full: Value = serde_json::from_str(&plan(&next, None).payload).unwrap();
            assert_eq!(full["input"], next["input"]);

            let mut changed = next.clone();
            changed["input"][1][field] = populated.clone();
            assert_full_history(&changed, &checkpoint);
        }
        let mut populated_item = item;
        populated_item[field] = populated;
        let checkpoint = checkpoint(&body, &json!([populated_item]));
        let mut next = body.clone();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([null, json!({"role":"user","content":"synthetic next"})]);
        assert_full_history(&next, &checkpoint);
    }
}

#[test]
fn optional_field_normalization_is_scoped_to_the_owning_item_type() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for kind in [
        "reasoning",
        "function_call_output",
        "custom_tool_call_output",
        "synthetic_extension",
    ] {
        for field in ["namespace", "encrypted_function_args", "async"] {
            let output = json!({"type":kind,"id":"synthetic","output":"synthetic"});
            let checkpoint = checkpoint(&body, &json!([output]));
            let mut replay = output;
            replay[field] = Value::Null;
            let mut next = body.clone();
            next["input"]
                .as_array_mut()
                .unwrap()
                .extend([replay, json!({"role":"user","content":"synthetic next"})]);
            assert_full_history(&next, &checkpoint);
        }
    }
}
