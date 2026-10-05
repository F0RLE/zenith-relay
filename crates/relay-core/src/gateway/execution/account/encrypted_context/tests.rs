use super::drop_rejected_encrypted_context;
use serde_json::json;

#[test]
fn rejected_ciphertext_is_removed_without_losing_visible_or_ordinary_history() {
    let mut request = json!({
        "input": [
            {
                "id": "rs_foreign",
                "type": "reasoning",
                "encrypted_content": "synthetic-reasoning-ciphertext",
                "summary": [{"type": "summary_text", "text": "visible reasoning"}]
            },
            {"id": "cmp_foreign", "type": "compaction", "encrypted_content": {"blob": "synthetic-compaction-ciphertext"}},
            {"id": "cmp_summary", "type": "compaction_summary", "encrypted_content": "synthetic-summary-ciphertext", "content": [{"type": "summary_text", "text": "visible compaction"}]},
            {"id": "rs_untyped", "encrypted_content": "synthetic-untyped-ciphertext"},
            {"id": "rs_empty", "type": "reasoning", "encrypted_content": "  ", "summary": []},
            {"id": "cmp_plain", "type": "compaction", "summary": []},
            {"role": "assistant", "content": [{"type": "output_text", "text": "previous answer"}]},
            {"type": "function_call_output", "call_id": "call_1", "output": "done"}
        ]
    });

    assert!(drop_rejected_encrypted_context(&mut request));
    assert!(!drop_rejected_encrypted_context(&mut request));

    let items = request["input"].as_array().unwrap();
    let reasoning = items
        .iter()
        .find(|item| item.pointer("/summary/0/text") == Some(&json!("visible reasoning")))
        .expect("visible reasoning summary should remain");
    assert_eq!(reasoning["type"], "reasoning");
    assert!(reasoning.get("encrypted_content").is_none());
    assert!(reasoning.get("id").is_none());

    let compaction = items
        .iter()
        .find(|item| item.pointer("/content/0/text") == Some(&json!("visible compaction")))
        .expect("visible compaction summary should remain");
    assert!(compaction.get("encrypted_content").is_none());
    assert!(compaction.get("id").is_none());

    assert!(items.iter().any(|item| item["id"] == "rs_empty"));
    assert!(items.iter().any(|item| item["id"] == "cmp_plain"));
    assert!(items.iter().any(|item| item["role"] == "assistant"));
    assert!(items
        .iter()
        .any(|item| item["type"] == "function_call_output"));
    assert!(items
        .iter()
        .all(|item| item.get("id").and_then(serde_json::Value::as_str) != Some("cmp_foreign")));
    assert!(items
        .iter()
        .all(|item| item.get("id").and_then(serde_json::Value::as_str) != Some("rs_untyped")));
}

#[test]
fn request_without_responses_input_array_is_unchanged() {
    let mut request = json!({"input": "continue"});
    assert!(!drop_rejected_encrypted_context(&mut request));
    assert_eq!(request["input"], "continue");
}
