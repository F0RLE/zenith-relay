use super::*;

fn context() -> AgentMessageContext {
    AgentMessageContext::new(Arc::default(), "local-key")
}

#[test]
fn sealed_argument_roundtrips_as_lower_authority_context_and_history() {
    let context = context();
    let text = "Synthetic task\nwith Unicode: проверка";
    let sealed = context
        .seal_argument("collaboration.spawn_agent", "message", text)
        .unwrap();
    assert!(!sealed.contains("Synthetic"));
    let mut request = json!({"input":[{"type":"agent_message","author":"/root","recipient":"/root/worker","role":"developer",
        "content":[{"type":"encrypted_content","encrypted_content":sealed}]}]});
    context.restore_request(&mut request).unwrap();
    assert_eq!(request["input"][0]["role"], "user");
    assert_eq!(request["input"][0]["content"][1]["text"], text);
    assert!(
        request["input"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("/root/worker")
    );
    let mut history = json!({"input":[{"type":"function_call","namespace":"collaboration","name":"spawn_agent","call_id":"call_synthetic",
        "arguments":json!({"message":sealed,"task_name":"worker"}).to_string()}]});
    context.restore_request(&mut history).unwrap();
    let arguments: Value =
        serde_json::from_str(history["input"][0]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments["message"], text);
    assert_eq!(arguments["task_name"], "worker");
}

#[test]
fn ciphertext_is_key_scoped_authenticated_and_host_scoped() {
    let context = context();
    let sealed = context
        .seal_argument("spawn", "message", "synthetic")
        .unwrap();
    let request = json!({"input":{"type":"agent_message","encrypted_content":sealed}});
    let other_key = AgentMessageContext::new(context.cipher.clone(), "other-key");
    assert!(other_key.restore_request(&mut request.clone()).is_err());
    assert!(
        self::context()
            .restore_request(&mut request.clone())
            .is_err()
    );
    let mut corrupted = sealed.into_bytes();
    let index = PREFIX.len() + 20;
    corrupted[index] = if corrupted[index] == b'A' { b'B' } else { b'A' };
    let mut tampered = json!({"input":{"type":"agent_message","encrypted_content":String::from_utf8(corrupted).unwrap()}});
    assert!(context.restore_request(&mut tampered).is_err());
    context
        .clone()
        .restore_request(&mut request.clone())
        .unwrap();
}

#[test]
fn foreign_ciphertext_and_unrelated_payloads_are_not_interpreted() {
    let context = context();
    let mut request = json!({"input":[
        {"type":"agent_message","content":[{"type":"encrypted_content","encrypted_content":"provider-cipher"}]},
        {"type":"reasoning","encrypted_content":"provider-reasoning"},
        {"type":"function_call_output","call_id":"call_synthetic","output":"zenith-relay-agent-v1:not-a-message"}
    ]});
    let original = request.clone();
    context.restore_request(&mut request).unwrap();
    assert_eq!(request, original);
}

#[test]
fn lost_host_key_reports_unavailable_history_without_exposing_the_payload() {
    let original = context();
    let sealed = original
        .seal_argument(
            "collaboration.send_message",
            "message",
            "synthetic private task",
        )
        .unwrap();
    let request = json!({"input":[{"type":"agent_message","encrypted_content":sealed}]});
    for receiver in [
        context(),
        AgentMessageContext::new(original.cipher.clone(), "another-key"),
    ] {
        let error = receiver.restore_request(&mut request.clone()).unwrap_err();
        assert_eq!(error, AdapterError::agent_message_unavailable());
        assert_eq!(
            error.parameter(),
            Some("input.agent_message.encrypted_content")
        );
        assert!(!error.is_route_incompatible());
        assert!(!error.is_upstream_failure());
        let message = error.to_string();
        assert!(message.contains("original profile and key"));
        assert!(message.contains("start a new chat"));
        assert!(!message.contains("synthetic private task"));
        assert!(!message.contains(&sealed));
    }
}

#[test]
fn historical_arguments_cannot_move_between_tools_or_parameters() {
    let context = context();
    let sealed = context
        .seal_argument("spawn", "message", "synthetic")
        .unwrap();
    for (name, property) in [("other", "message"), ("spawn", "cmd")] {
        let mut request = json!({"input":[{"type":"function_call","name":name,
            "arguments":json!({property:sealed}).to_string()}]});
        assert!(context.restore_request(&mut request).is_err());
    }
}

#[test]
fn oversized_and_malformed_carriers_fail_closed() {
    let context = context();
    assert!(
        context
            .seal_argument("spawn", "message", &"x".repeat(MAX_TEXT_BYTES + 1))
            .is_err()
    );
    for blob in [
        format!("{PREFIX}invalid"),
        format!("{PREFIX}{}", "x".repeat(MAX_ENVELOPE_BYTES + 1)),
    ] {
        let mut request = json!({"input":{"type":"agent_message","encrypted_content":blob}});
        assert!(context.restore_request(&mut request).is_err());
    }
}

#[test]
fn mixed_parts_keep_order_but_mixed_foreign_ciphertext_is_not_silently_dropped() {
    let context = context();
    let first = context
        .seal_argument("delegate", "message", "first")
        .unwrap();
    let last = context
        .seal_argument("delegate", "message", "last")
        .unwrap();
    let mut request = json!({"input":{"type":"agent_message","author":"/root","content":[
        {"type":"encrypted_content","encrypted_content":first},
        {"type":"input_text","text":"middle"},
        {"type":"encrypted_content","encrypted_content":last}
    ]}});
    context.restore_request(&mut request).unwrap();
    let content = request["input"]["content"].as_array().unwrap();
    assert_eq!(content[1]["text"], "first");
    assert_eq!(content[2]["text"], "middle");
    assert_eq!(content[3]["text"], "last");
    let mut mixed = json!({"input":{"type":"agent_message","encrypted_content":first,"content":[
        {"type":"encrypted_content","encrypted_content":"foreign-ciphertext"}
    ]}});
    assert!(context.restore_request(&mut mixed).is_err());
}

#[test]
fn maximum_escaped_plaintext_fits_the_authenticated_envelope_limit() {
    let context = context();
    let text = "\u{0001}".repeat(MAX_TEXT_BYTES);
    let sealed = context.seal_argument("delegate", "message", &text).unwrap();
    let mut request = json!({"input":{"type":"agent_message","encrypted_content":sealed}});
    context.restore_request(&mut request).unwrap();
    assert_eq!(request["input"]["content"][1]["text"], text);
}
