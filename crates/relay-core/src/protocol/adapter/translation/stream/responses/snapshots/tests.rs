use super::*;
use serde_json::json;

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
