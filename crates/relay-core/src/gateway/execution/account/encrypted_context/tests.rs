use super::drop_rejected_encrypted_context as repair;
use serde_json::json;

fn drop_rejected_encrypted_context(request: &mut serde_json::Value) -> bool {
    repair(
        request,
        br#"{"error":{"code":"invalid_encrypted_content"}}"#,
    )
}

#[test]
fn provider_locator_repairs_only_the_rejected_item_and_keeps_tool_history() {
    for diagnostic in [
        json!({"param":"input[1].encrypted_content"}),
        json!({"message":"The encrypted content BBBB...2222 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}),
        json!({"param":"input[1].encrypted_content", "message":"The encrypted content BBBB...2222 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}),
    ] {
        let mut request = json!({"input":[
            {"id":"rs_first","type":"reasoning","encrypted_content":"AAAA-synthetic-1111","summary":[]},
            {"id":"rs_second","type":"reasoning","encrypted_content":"BBBB-synthetic-2222","summary":[{"text":"Visible summary"}]},
            {"type":"function_call","name":"synthetic_tool","call_id":"call_1","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_1","output":"Synthetic output"}
        ]});
        let original = request.clone();
        let mut error = diagnostic;
        error["code"] = json!("invalid_encrypted_content");
        let body = serde_json::to_vec(&json!({"error":error})).unwrap();
        assert!(repair(&mut request, &body));
        assert_eq!(request["input"][0], original["input"][0]);
        assert_eq!(
            request["input"][1],
            json!({"type":"reasoning","summary":[{"text":"Visible summary"}]})
        );
        assert_eq!(request["input"][2], original["input"][2]);
        assert_eq!(request["input"][3], original["input"][3]);
    }
}

#[test]
fn unsafe_or_conflicting_locators_leave_all_history_unchanged() {
    let original = json!({"input":[
        {"id":"rs_first","type":"reasoning","encrypted_content":"AAAA-synthetic-1111","summary":[]},
        {"id":"rs_second","type":"reasoning","encrypted_content":"BBBB-synthetic-2222","summary":[]}
    ]});
    for error in [
        json!({"code":"invalid_encrypted_content"}),
        json!({"code":"invalid_encrypted_content","param":"input[9].encrypted_content"}),
        json!({"code":"invalid_encrypted_content","param":"input[01].encrypted_content"}),
        json!({"code":"invalid_encrypted_content","param":"input[0]"}),
        json!({"code":"invalid_encrypted_content","message":"The encrypted content BBBB…2222 could not be verified."}),
        json!({"code":"invalid_encrypted_content","param":"input[0].encrypted_content", "message":"The encrypted content BBBB...2222 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}),
        json!({"code":"thinking_signature_invalid","param":"input[1].encrypted_content"}),
    ] {
        let mut request = original.clone();
        assert!(!repair(
            &mut request,
            &serde_json::to_vec(&json!({"error":error})).unwrap()
        ));
        assert_eq!(request, original);
    }
    for (field, value) in [
        ("id", "rs_first"),
        ("encrypted_content", "AAAA-synthetic-1111"),
    ] {
        let mut request = original.clone();
        request["input"][1][field] = json!(value);
        let before = request.clone();
        let body = br#"{"error":{"code":"invalid_encrypted_content","message":"The encrypted content AAAA...1111 could not be verified. Reason: Encrypted content could not be decrypted or parsed."}}"#;
        assert!(!repair(&mut request, body));
        assert_eq!(request, before);
    }
}

#[test]
fn malformed_rejection_envelopes_cannot_authorize_history_edits() {
    let original = json!({"input":[
        {"id":"rs_only","type":"reasoning","encrypted_content":"synthetic","summary":[]}
    ]});
    for body in [
        br#"{"error":{"code":"invalid_encrypted_content","param":"input[0].encrypted_content","param":"input[1].encrypted_content"}}"#.as_slice(),
        br#"{"error":{"code":"invalid_encrypted_content","code":"invalid_encrypted_content"}}"#,
        br#"{"code":"invalid_encrypted_content","error":{"code":"invalid_encrypted_content"}}"#,
        br#"{"error":{"code":"invalid_encrypted_content","message":{}}}"#,
        br#"{"error":{"message":"invalid_encrypted_content"}}"#,
        b"not JSON",
    ] {
        let mut request = original.clone();
        assert!(!repair(&mut request, body));
        assert_eq!(request, original);
    }
    let mut request = original.clone();
    assert!(!repair(&mut request, &vec![b' '; 65_537]));
    assert_eq!(request, original);
}

#[test]
fn exact_legacy_item_diagnostic_preserves_compatibility_without_guessing() {
    let original = json!({"input":[
        {"id":"rs_first","type":"reasoning","encrypted_content":"synthetic-first","summary":[]},
        {"id":"rs_second","type":"reasoning","encrypted_content":"synthetic-second","summary":[]}
    ]});
    let message = "Encrypted content for item rs_second could not be verified. Reason: Encrypted content could not be decrypted or parsed.";
    let mut request = original.clone();
    let body = serde_json::to_vec(&json!({"error":{"message":message}})).unwrap();
    assert!(repair(&mut request, &body));
    assert_eq!(request["input"], json!([original["input"][0]]));
    for error in [
        json!({"code":"other_error","message":message}),
        json!({"message":message,"param":"input[0].encrypted_content"}),
        json!({"message":"Encrypted content for item rs_missing could not be verified. Reason: Encrypted content could not be decrypted or parsed."}),
        json!({"message":"Encrypted content for item rs_second could not be verified."}),
    ] {
        let mut request = original.clone();
        assert!(!repair(
            &mut request,
            &serde_json::to_vec(&json!({"error":error})).unwrap()
        ));
        assert_eq!(request, original);
    }
}

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

    assert!(items.iter().any(|item| item["id"] == "rs_empty"));
    assert!(items.iter().any(|item| item["id"] == "cmp_plain"));
    assert!(items.iter().any(|item| item["role"] == "assistant"));
    assert!(items
        .iter()
        .any(|item| item["type"] == "function_call_output"));
}

#[test]
fn ambiguous_rejected_ciphertext_preserves_the_entire_request() {
    let mut request = json!({
        "previous_response_id": "resp_original",
        "input": [
            {"id":"rs_first", "type":"reasoning", "encrypted_content":"synthetic-first", "summary":[]},
            {"id":"rs_second", "encrypted_content":"synthetic-second"},
            {"type":"function_call", "call_id":"call_original", "name":"inspect", "arguments":"{}"},
            {"type":"function_call_output", "call_id":"call_original", "output":"synthetic-result"},
            {"role":"user", "content":"Continue"}
        ]
    });
    let original = request.clone();
    assert!(!drop_rejected_encrypted_context(&mut request));
    assert_eq!(request, original);
}

#[test]
fn a_single_untyped_reasoning_item_can_be_removed_after_rejection() {
    let mut request = json!({"input":[
        {"id":"rs_untyped", "encrypted_content":"synthetic-ciphertext"},
        {"role":"user", "content":"Continue"}
    ]});
    assert!(drop_rejected_encrypted_context(&mut request));
    assert_eq!(
        request["input"],
        json!([{"role":"user", "content":"Continue"}])
    );
}

#[test]
fn encrypted_compaction_blocks_repair_without_changing_any_history() {
    for checkpoint in [
        json!({"id":"cmp_foreign", "type":"compaction", "encrypted_content":{"blob":"synthetic"}}),
        json!({"type":"compaction_summary", "encrypted_content":"synthetic", "content":[{"type":"summary_text", "text":"visible summary"}]}),
        json!({"id":"cmp_untyped", "encrypted_content":"synthetic"}),
    ] {
        let mut request = json!({
            "previous_response_id": "resp_original",
            "input": [
                {"type":"reasoning", "encrypted_content":"synthetic", "summary":[]},
                checkpoint,
                {"role":"user", "content":"Continue"}
            ]
        });
        let original = request.clone();
        assert!(!drop_rejected_encrypted_context(&mut request));
        assert_eq!(request, original);
    }
}

#[test]
fn request_without_responses_input_array_is_unchanged() {
    let mut request = json!({"input": "continue"});
    assert!(!drop_rejected_encrypted_context(&mut request));
    assert_eq!(request["input"], "continue");
}
