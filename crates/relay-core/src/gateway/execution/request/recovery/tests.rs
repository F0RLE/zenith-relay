use super::*;

#[test]
fn adapter_failure_preserves_usage_and_static_reason_without_overwriting_provider_error() {
    let mut event = crate::gateway::test_support::test_usage_event();
    event.http_status = 200;
    event.input_tokens = Some(17);
    event.output_tokens = Some(4);
    let error =
        AdapterError::upstream_response_invalid().with_parameter("output.run_officejs.args");
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
    let marked_again = mark_adapter_failure(
        marked.clone(),
        &AdapterError::upstream_stream_invalid().with_parameter("stream.terminal"),
    );
    assert_eq!(marked_again.upstream_error, marked.upstream_error);
}
