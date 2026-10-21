use super::*;

#[test]
fn adapter_failure_preserves_usage_and_static_reason_without_overwriting_provider_error() {
    let mut event = crate::gateway::test_support::test_usage_event();
    event.http_status = 200;
    event.input_tokens = Some(17);
    event.output_tokens = Some(4);
    let error = AdapterError::upstream_response_invalid()
        .with_parameter("output.run_officejs.args")
        .with_json_diagnostic("invalid_escape", "json_object", 1, 9, 20);
    let marked = mark_adapter_failure(event, &error);
    assert!(!marked.success);
    assert_eq!(marked.http_status, 502);
    assert_eq!(marked.input_tokens, Some(17));
    assert_eq!(marked.output_tokens, Some(4));
    let details = marked.upstream_error.as_ref().unwrap();
    assert_eq!(details.http_status, Some(200));
    assert_eq!(details.error_type.as_deref(), Some("relay_adapter"));
    assert!(details
        .message
        .as_ref()
        .unwrap()
        .contains("output.run_officejs.args"));
    assert!(details
        .message
        .as_ref()
        .unwrap()
        .contains("json_reason=invalid_escape"));
    assert!(details.message.as_ref().unwrap().contains("parse_column=9"));
    assert!(details
        .message
        .as_ref()
        .unwrap()
        .contains("failure_stage=tool_arguments"));
    assert!(details
        .message
        .as_ref()
        .unwrap()
        .contains("repair_skipped=executable_tool"));
    let marked_again = mark_adapter_failure(
        marked.clone(),
        &AdapterError::upstream_stream_invalid().with_parameter("stream.terminal"),
    );
    assert_eq!(marked_again.upstream_error, marked.upstream_error);
}

#[test]
fn bps_failure_stages_are_static_and_do_not_enable_tool_regeneration() {
    for (parameter, stage) in [
        ("output.run_officejs.args.schema", "tool_arguments"),
        ("output.run_officejs.tool", "tool_identity"),
        ("output.run_officejs.call_id", "tool_identity"),
        ("output.run_officejs.references", "tool_envelope"),
    ] {
        let error = AdapterError::upstream_response_invalid().with_parameter(parameter);
        let rendered = error.to_string();
        assert!(rendered.contains(&format!("failure_stage={stage}")));
        assert!(rendered.contains("repair_skipped=executable_tool"));
        assert_eq!(
            error.code(),
            crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID
        );
    }
    assert!(!AdapterError::upstream_stream_invalid()
        .to_string()
        .contains("failure_stage="));
}
