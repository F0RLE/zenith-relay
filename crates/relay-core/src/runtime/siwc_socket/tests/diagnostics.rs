use super::*;
use crate::usage::{FullRequestReason, SocketReplayField, SocketReplayItemKind};

fn checkpoint(body: &Value, output: &Value, diagnostics: bool) -> planner::Checkpoint {
    planner::parse(&serde_json::to_vec(body).unwrap())
        .unwrap()
        .with_diagnostics(diagnostics)
        .plan(None)
        .state
        .complete(&json!({"id":"synthetic-response","status":"completed","output":output}))
        .unwrap()
}

#[test]
fn output_mismatch_reports_only_fixed_field_labels_and_relative_index() {
    let body = request(json!([{"role":"user","content":"synthetic private input"}]));
    let reasoning = json!({"type":"reasoning","id":"synthetic-reasoning-id",
        "summary":[],"encrypted_content":"synthetic-private-cipher"});
    let message = json!({"type":"message","id":"synthetic-message-id","role":"assistant",
        "content":[{"type":"output_text","text":"synthetic private reply"}],
        "synthetic-private-extension-key":"synthetic private extension value"});
    let checkpoint = checkpoint(&body, &json!([reasoning, message]), true);
    let mut edited = message;
    edited["id"] = json!("synthetic-edited-id");
    edited["content"][0]["text"] = json!("synthetic edited text");
    edited
        .as_object_mut()
        .unwrap()
        .remove("synthetic-private-extension-key");
    let mut next = body;
    next["input"].as_array_mut().unwrap().extend([
        reasoning,
        edited,
        json!({"role":"user","content":"synthetic next"}),
    ]);
    let planned = plan(&next, Some(&checkpoint));
    assert_eq!(
        planned.full_request_reason,
        Some(FullRequestReason::SocketOutputChanged)
    );
    let mismatch = planned.output_mismatch.unwrap();
    assert_eq!(mismatch.output_index, 1);
    assert_eq!(
        mismatch.fields,
        vec![
            SocketReplayField::Id,
            SocketReplayField::Content,
            SocketReplayField::Other
        ]
    );
    assert_eq!(
        serde_json::to_value(&mismatch).unwrap(),
        json!({
            "outputIndex":1,"fields":["id","content","other"],
            "itemKind":"message", "unknownFields":{
                "added":0,"removed":1,"changed":0,"nullOnly":false,"truncated":false
            }
        })
    );
    let encoded = serde_json::to_string(&mismatch).unwrap();
    assert!(!encoded.contains("synthetic"));
    let full: Value = serde_json::from_str(&planned.payload).unwrap();
    assert_eq!(full["input"], next["input"]);
    assert!(full.get("previous_response_id").is_none());
}

#[test]
fn documented_semantic_fields_are_identified_without_relaxing_continuation() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for (kind, field, value, expected) in [
        (
            "tool_search_call",
            "execution",
            json!("client"),
            SocketReplayField::Execution,
        ),
        (
            "tool_search_output",
            "tools",
            json!([]),
            SocketReplayField::Tools,
        ),
        (
            "web_search_call",
            "action",
            json!({"type":"search","query":"synthetic private query"}),
            SocketReplayField::Action,
        ),
        (
            "agent_message",
            "author",
            json!("synthetic private author"),
            SocketReplayField::Author,
        ),
        (
            "agent_message",
            "recipient",
            json!("synthetic private recipient"),
            SocketReplayField::Recipient,
        ),
        (
            "image_generation_call",
            "result",
            json!("synthetic private image"),
            SocketReplayField::Result,
        ),
        (
            "image_generation_call",
            "revised_prompt",
            json!("synthetic private prompt"),
            SocketReplayField::RevisedPrompt,
        ),
    ] {
        let item = json!({"type":kind,"id":"synthetic"});
        let checkpoint = checkpoint(&body, &json!([item]), true);
        let mut edited = item;
        edited[field] = value;
        let mut next = body.clone();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([edited, json!({"role":"user","content":"synthetic next"})]);
        let planned = plan(&next, Some(&checkpoint));
        assert_eq!(
            planned.full_request_reason,
            Some(FullRequestReason::SocketOutputChanged)
        );
        let mismatch = planned.output_mismatch.unwrap();
        assert_eq!(mismatch.fields, vec![expected]);
        assert_eq!(mismatch.unknown_fields, None);
        assert!(
            !serde_json::to_string(&mismatch)
                .unwrap()
                .contains("synthetic")
        );
    }
}

#[test]
fn unknown_extension_changes_are_bounded_content_free_and_never_authorize_replay() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let item = json!({"type":"reasoning","id":"synthetic","summary":[],
        "synthetic-removed":null,"synthetic-changed":"synthetic private before"});
    let checkpoint = checkpoint(&body, &json!([item]), true);
    for null_only in [false, true] {
        let mut edited = item.clone();
        edited.as_object_mut().unwrap().remove("synthetic-removed");
        edited["synthetic-added"] = Value::Null;
        if !null_only {
            edited["synthetic-changed"] = json!("synthetic private after");
        }
        let mut next = body.clone();
        next["input"]
            .as_array_mut()
            .unwrap()
            .extend([edited, json!({"role":"user","content":"synthetic next"})]);
        let planned = plan(&next, Some(&checkpoint));
        assert_eq!(
            planned.full_request_reason,
            Some(FullRequestReason::SocketOutputChanged)
        );
        let mismatch = planned.output_mismatch.unwrap();
        assert_eq!(mismatch.item_kind, Some(SocketReplayItemKind::Reasoning));
        assert_eq!(mismatch.fields, vec![SocketReplayField::Other]);
        let unknown = mismatch.unknown_fields.unwrap();
        assert_eq!(unknown.added, 1);
        assert_eq!(unknown.removed, 1);
        assert_eq!(unknown.changed, u32::from(!null_only));
        assert_eq!(unknown.null_only, null_only);
        assert!(!unknown.truncated);
        assert!(
            !serde_json::to_string(&mismatch)
                .unwrap()
                .contains("synthetic")
        );
        let full: Value = serde_json::from_str(&planned.payload).unwrap();
        assert_eq!(full["input"], next["input"]);
    }
}

#[test]
fn truncated_unknown_proofs_cannot_claim_null_only_or_change_replay_decisions() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let mut item = json!({"type":"reasoning","id":"synthetic","summary":[]});
    for index in 0..17 {
        item[format!("synthetic-private-field-{index}")] = Value::Null;
    }
    let checkpoint = checkpoint(&body, &json!([item]), true);
    let mut next = body;
    next["input"].as_array_mut().unwrap().extend([
        json!({"type":"reasoning","id":"synthetic","summary":[]}),
        json!({"role":"user","content":"synthetic next"}),
    ]);
    let planned = plan(&next, Some(&checkpoint));
    assert_eq!(
        planned.full_request_reason,
        Some(FullRequestReason::SocketOutputChanged)
    );
    let mismatch = planned.output_mismatch.unwrap();
    let unknown = mismatch.unknown_fields.unwrap();
    assert_eq!(unknown.removed, 16);
    assert!(unknown.truncated);
    assert!(!unknown.null_only);
    assert!(
        !serde_json::to_string(&mismatch)
            .unwrap()
            .contains("synthetic")
    );
}

#[test]
fn bounded_unknown_proofs_do_not_invent_changes_when_json_key_order_differs() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let mut item = json!({"type":"reasoning","id":"synthetic","summary":[]});
    for index in 0..20 {
        item[format!("synthetic-private-field-{index}")] = Value::Null;
    }
    let checkpoint = checkpoint(&body, &json!([item]), true);
    let mut edited = serde_json::Map::new();
    for (name, value) in item.as_object().unwrap().iter().rev() {
        edited.insert(name.clone(), value.clone());
    }
    edited.insert("status".into(), json!("incomplete"));
    edited.insert("synthetic-private-field-20".into(), json!(true));
    let mut next = body;
    next["input"].as_array_mut().unwrap().extend([
        Value::Object(edited),
        json!({"role":"user","content":"synthetic next"}),
    ]);
    let planned = plan(&next, Some(&checkpoint));
    assert_eq!(
        planned.full_request_reason,
        Some(FullRequestReason::SocketOutputChanged)
    );
    let mismatch = planned.output_mismatch.unwrap();
    let unknown = mismatch.unknown_fields.unwrap();
    assert_eq!(unknown.changed, 0);
    assert!(unknown.added <= 1);
    assert_eq!(unknown.removed, 0);
    assert!(unknown.truncated);
    assert!(!unknown.null_only);
    let full: Value = serde_json::from_str(&planned.payload).unwrap();
    assert_eq!(full["input"], next["input"]);
}

#[test]
fn field_proofs_are_optional_bounded_and_never_authorize_changed_history() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let output: Vec<_> = (0..33)
        .map(|index| {
            json!({"type":"message","id":format!("msg_{index}"),
        "role":"assistant","content":[{"type":"output_text","text":"synthetic"}]})
        })
        .collect();
    let mut next = body.clone();
    next["input"].as_array_mut().unwrap().extend(output.clone());
    next["input"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"user","content":"next"}));
    for diagnostics in [false, true] {
        let checkpoint = checkpoint(&body, &json!(output), diagnostics);
        for index in [31, 32] {
            let mut changed = next.clone();
            changed["input"][index + 1]["content"][0]["text"] = json!("synthetic edited");
            let planned = plan(&changed, Some(&checkpoint));
            assert_eq!(
                planned.full_request_reason,
                Some(FullRequestReason::SocketOutputChanged)
            );
            assert_eq!(planned.output_mismatch.is_some(), diagnostics && index < 32);
            if let Some(mismatch) = planned.output_mismatch {
                assert_eq!(mismatch.output_index, index as u32);
                assert_eq!(mismatch.fields, vec![SocketReplayField::Content]);
            }
        }
        let disabled = planner::parse(&serde_json::to_vec(&next).unwrap())
            .unwrap()
            .plan(Some(&checkpoint));
        assert!(disabled.output_mismatch.is_none());
    }
}

#[test]
fn execution_and_encryption_fields_use_fixed_labels_without_exposing_values() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    for (field, value, expected) in [
        ("async", json!(true), SocketReplayField::Async),
        (
            "encrypted_function_args",
            json!(["synthetic-private-cipher"]),
            SocketReplayField::EncryptedFunctionArgs,
        ),
    ] {
        let call = json!({"type":"function_call","id":"fc_test","call_id":"call_test",
            "name":"lookup","arguments":"{}"});
        let checkpoint = checkpoint(&body, &json!([call]), true);
        let mut replay = call;
        replay[field] = value;
        let mut next = body.clone();
        next["input"].as_array_mut().unwrap().extend([
            replay,
            json!({"type":"function_call_output","call_id":"call_test","output":"synthetic"}),
        ]);
        let planned = plan(&next, Some(&checkpoint));
        assert_eq!(
            planned.full_request_reason,
            Some(FullRequestReason::SocketOutputChanged)
        );
        let mismatch = planned.output_mismatch.unwrap();
        assert_eq!(mismatch.fields, vec![expected]);
        assert!(
            !serde_json::to_string(&mismatch)
                .unwrap()
                .contains("synthetic")
        );
    }
}

#[test]
fn missing_fields_are_classified_without_exposing_values_or_weakening_other_reasons() {
    let body = request(json!([{"role":"user","content":"synthetic"}]));
    let call = json!({"type":"function_call","id":"fc_test","call_id":"call_test",
        "name":"lookup","arguments":"{\"synthetic-private-key\":1}"});
    let checkpoint = checkpoint(&body, &json!([call]), true);
    let mut replay = call;
    replay.as_object_mut().unwrap().remove("arguments");
    let mut next = body;
    next["input"].as_array_mut().unwrap().extend([
        replay,
        json!({"type":"function_call_output","call_id":"call_test","output":"synthetic"}),
    ]);
    assert_eq!(
        plan(&next, Some(&checkpoint))
            .output_mismatch
            .unwrap()
            .fields,
        vec![SocketReplayField::Arguments]
    );
    next["input"][0]["content"] = json!("synthetic changed prefix");
    let history = plan(&next, Some(&checkpoint));
    assert_eq!(
        history.full_request_reason,
        Some(FullRequestReason::SocketHistoryChanged)
    );
    assert!(history.output_mismatch.is_none());
    next["model"] = json!("synthetic-other-model");
    let settings = plan(&next, Some(&checkpoint));
    assert_eq!(
        settings.full_request_reason,
        Some(FullRequestReason::SocketSettingsChanged)
    );
    assert!(settings.output_mismatch.is_none());
}
