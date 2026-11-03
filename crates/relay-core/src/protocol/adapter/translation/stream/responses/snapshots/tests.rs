use super::*;
use serde_json::json;

#[test]
fn completed_reasoning_keeps_done_ciphertext_without_rewriting_other_fields() {
    let previous = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
        "status":"completed","encrypted_content":"synthetic-done-cipher"});
    for replacement in [
        None,
        Some(Value::Null),
        Some(json!("synthetic-terminal-cipher")),
    ] {
        let mut terminal = previous.clone();
        terminal
            .as_object_mut()
            .unwrap()
            .remove("encrypted_content");
        if let Some(replacement) = replacement {
            terminal["encrypted_content"] = replacement;
        }
        retain_completed_reasoning(&previous, &mut terminal).unwrap();
        assert_eq!(terminal, previous);
        validate_unchanged(&previous, &terminal).unwrap();
    }
    for (field, value) in [
        (
            "summary",
            json!([{"type":"summary_text","text":"synthetic-private"}]),
        ),
        ("status", json!("incomplete")),
        ("id", json!("rs_other")),
    ] {
        let mut terminal = previous.clone();
        terminal["encrypted_content"] = json!("synthetic-terminal-cipher");
        terminal[field] = value.clone();
        retain_completed_reasoning(&previous, &mut terminal).unwrap();
        assert_eq!(terminal[field], value);
        let error = validate_unchanged(&previous, &terminal).unwrap_err();
        assert!(!error.to_string().contains("synthetic-private"));
    }
}

#[test]
fn reasoning_reconciliation_rejects_invalid_cipher_types_and_never_invents_done_state() {
    let previous = json!({"type":"reasoning","id":"rs_synthetic","summary":[],
        "encrypted_content":"synthetic-done-cipher"});
    for invalid in [
        json!(false),
        json!(42),
        json!([]),
        json!({"synthetic-private":true}),
    ] {
        let mut terminal = previous.clone();
        terminal["encrypted_content"] = invalid;
        let error = retain_completed_reasoning(&previous, &mut terminal).unwrap_err();
        assert_eq!(
            error.parameter(),
            Some("response.output_item.encrypted_content")
        );
        assert!(!error.to_string().contains("synthetic-private"));
    }
    for absent in [None, Some(Value::Null)] {
        let mut previous = previous.clone();
        previous
            .as_object_mut()
            .unwrap()
            .remove("encrypted_content");
        if let Some(absent) = absent {
            previous["encrypted_content"] = absent;
        }
        let mut terminal = previous.clone();
        terminal["encrypted_content"] = json!("synthetic-terminal-cipher");
        let unchanged = terminal.clone();
        retain_completed_reasoning(&previous, &mut terminal).unwrap();
        assert_eq!(terminal, unchanged);
        assert!(validate_unchanged(&previous, &terminal).is_err());
    }
}

#[test]
fn only_documented_nullable_item_fields_allow_absent_and_null() {
    for (kind, field) in [
        ("message", "phase"),
        ("reasoning", "encrypted_content"),
        ("function_call", "caller"),
        ("custom_tool_call", "caller"),
    ] {
        let absent = json!({"type":kind,"id":"synthetic_item"});
        let mut null = absent.clone();
        null[field] = Value::Null;
        assert!(validate_unchanged(&absent, &null).is_ok());
        assert!(validate_unchanged(&null, &absent).is_ok());
        null[field] = json!("synthetic-value");
        assert!(validate_unchanged(&absent, &null).is_err());
        assert!(validate_unchanged(&null, &absent).is_err());
    }
    let unchanged = json!({"type":"message","content":[{"type":"output_text","text":"synthetic"}]});
    assert!(validate_unchanged(&unchanged, &unchanged).is_ok());
}

#[test]
fn completed_content_and_execution_changes_remain_errors_with_safe_field_names() {
    for (field, before, after, parameter) in [
        (
            "content",
            json!([{"text":"synthetic-a"}]),
            json!([{"text":"synthetic-b"}]),
            "response.output_item.content",
        ),
        (
            "phase",
            json!("commentary"),
            json!("final_answer"),
            "response.output_item.phase",
        ),
        (
            "encrypted_content",
            json!("synthetic-a"),
            json!("synthetic-b"),
            "response.output_item.encrypted_content",
        ),
        (
            "arguments",
            json!("{\"n\":1}"),
            json!("{\"n\":2}"),
            "response.output_item.arguments",
        ),
        (
            "status",
            json!("completed"),
            json!("incomplete"),
            "response.output_item.status",
        ),
        (
            "caller",
            Value::Null,
            json!({"type":"program","caller_id":"synthetic-private"}),
            "response.output_item.caller",
        ),
        (
            "synthetic-private",
            Value::Null,
            json!("synthetic-private"),
            "response.output_item.changed",
        ),
    ] {
        let kind = match field {
            "encrypted_content" => "reasoning",
            "arguments" | "caller" => "function_call",
            _ => "message",
        };
        let mut previous = json!({"type":kind});
        previous[field] = before;
        let mut terminal = previous.clone();
        terminal[field] = after;
        let error = validate_unchanged(&previous, &terminal).unwrap_err();
        assert_eq!(error.parameter(), Some(parameter));
        assert!(!error.to_string().contains("synthetic-private"));
    }
    let previous = json!({"type":"message"});
    let mut terminal = previous.clone();
    terminal["synthetic-private"] = Value::Null;
    assert_eq!(
        validate_unchanged(&previous, &terminal)
            .unwrap_err()
            .parameter(),
        Some("response.output_item.changed")
    );
}
