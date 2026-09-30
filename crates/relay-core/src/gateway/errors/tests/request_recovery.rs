use super::*;

#[test]
fn websocket_only_previous_response_errors_are_detected_without_matching_other_errors() {
    assert!(previous_response_requires_websocket(
            br#"{"error":{"message":"previous_response_id is only supported on Responses WebSocket v2"}}"#,
        ));
    assert!(!previous_response_requires_websocket(
        br#"{"error":{"message":"previous response with id resp_123 not found"}}"#,
    ));
    assert!(!previous_response_requires_websocket(
        br#"{"error":{"message":"WebSocket transport is unavailable"}}"#,
    ));
}

#[test]
fn invalid_function_call_output_call_ids_are_detected_without_matching_generic_errors() {
    assert!(responses_function_call_output_has_invalid_call_id(
        br#"{"error":{"message":"Invalid call_id for function_call_output"}}"#,
    ));
    assert!(responses_function_call_output_has_invalid_call_id(
        br#"{"error":{"code":"invalid_function_call_output_call_id"}}"#,
    ));
    assert!(!responses_function_call_output_has_invalid_call_id(
        br#"{"error":{"message":"Invalid call_id"}}"#,
    ));
    assert!(!responses_function_call_output_has_invalid_call_id(
        br#"Invalid call_id for function_call_output"#,
    ));
}

#[test]
fn missing_responses_call_ids_are_detected_without_matching_invalid_ids() {
    for payload in [
        br#"{"error":{"message":"Missing field call_id"}}"#.as_slice(),
        br#"{"error":{"message":"Missing required field: `call_id`"}}"#.as_slice(),
        br#"{"error":{"message":"The call id is required"}}"#.as_slice(),
        br#"{"error":{"code":"missing_call_id"}}"#.as_slice(),
        b"Missing required parameter: call_id".as_slice(),
    ] {
        assert!(
            responses_call_id_is_missing(payload),
            "expected missing call_id detector to match {}",
            String::from_utf8_lossy(payload)
        );
    }
    for payload in [
        br#"{"error":{"message":"Invalid call_id for function_call_output"}}"#.as_slice(),
        br#"{"error":{"message":"Invalid call id"}}"#.as_slice(),
        br#"{"error":{"message":"Missing field: model"}}"#.as_slice(),
        br#"{"error":{"code":"invalid_request"}}"#.as_slice(),
    ] {
        assert!(
            !responses_call_id_is_missing(payload),
            "unexpected missing call_id match for {}",
            String::from_utf8_lossy(payload)
        );
    }
}

#[test]
fn tool_link_rejection_is_consistent_for_json_and_stream_envelopes() {
    for message in [
        "Missing required field: call_id",
        "No tool output found for custom tool call call_test.",
        "No tool output found for function call call_test.",
        "Invalid call_id for function_call_output",
    ] {
        let error = json!({"message": message});
        let buffered = json!({"error": error});
        let stream = json!({"type":"response.failed", "response":{"error":error}});
        assert!(responses_tool_call_links_rejected(
            &serde_json::to_vec(&buffered).unwrap()
        ));
        assert!(responses_tool_call_links_rejected_value(&stream));
    }
    for message in [
        "Invalid request",
        "Missing field: model",
        "Invalid tool arguments",
    ] {
        let error = json!({"error":{"message":message}});
        assert!(!responses_tool_call_links_rejected(
            &serde_json::to_vec(&error).unwrap()
        ));
        assert!(!responses_tool_call_links_rejected_value(&error));
    }
}

#[test]
fn zenith_gateway_invalid_request_is_detected_without_matching_generic_bad_requests() {
    assert!(zenith_gateway_invalid_request(
        br#"{"error":{"code":"invalid_request","message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}"#,
    ));
    assert!(zenith_gateway_invalid_request(
        br#"{"type":"error","response":{"error":{"message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}}}"#,
    ));
    assert!(!zenith_gateway_invalid_request(
        br#"{"error":{"code":"invalid_request","message":"request payload is invalid"}}"#,
    ));
}

#[test]
fn strict_responses_function_item_id_error_is_detected_without_matching_call_id_errors() {
    assert!(responses_function_item_id_requires_fc_prefix(
            br#"{"error":{"message":"Invalid 'input[7].id': 'call_abc'. Expected an ID that begins with 'fc'."}}"#,
        ));
    assert!(!responses_function_item_id_requires_fc_prefix(
        br#"{"error":{"message":"Invalid call_id for function_call_output"}}"#,
    ));
    assert!(!responses_function_item_id_requires_fc_prefix(
        br#"{"error":{"message":"Expected an ID that begins with 'fc'."}}"#,
    ));
}

#[test]
fn strict_responses_custom_tool_item_id_error_is_detected_without_matching_function_errors() {
    assert!(responses_custom_tool_item_id_requires_ctc_prefix(
        br#"{"error":{"message":"Invalid 'input[433].id': 'fc_abc'. Expected an ID that begins with 'ctc'."}}"#,
    ));
    assert!(!responses_custom_tool_item_id_requires_ctc_prefix(
        br#"{"error":{"message":"Invalid 'input[7].id': 'call_abc'. Expected an ID that begins with 'fc'."}}"#,
    ));
    assert!(!responses_custom_tool_item_id_requires_ctc_prefix(
        br#"{"error":{"message":"Expected an ID that begins with 'ctc'."}}"#,
    ));
}

#[test]
fn strict_responses_message_item_id_error_is_detected_without_matching_other_item_errors() {
    assert!(responses_message_item_id_requires_msg_prefix(
            br#"{"error":{"message":"Invalid 'input[151].id': 'item_abc'. Expected an ID that begins with 'msg'."}}"#,
        ));
    assert!(!responses_message_item_id_requires_msg_prefix(
            br#"{"error":{"message":"Invalid 'input[7].id': 'call_abc'. Expected an ID that begins with 'fc'."}}"#,
        ));
    assert!(!responses_message_item_id_requires_msg_prefix(
        br#"{"error":{"message":"Expected an ID that begins with 'msg'."}}"#,
    ));
    assert!(responses_message_item_id_requires_msg_prefix(
        br#"{"error":{"message":"text part msg_Upmcar_gD7yF-8YP1qSM8AM not found"}}"#,
    ));
    assert!(!responses_message_item_id_requires_msg_prefix(
        br#"{"error":{"message":"text part fc_123 not found"}}"#,
    ));
}

#[test]
fn missing_tool_output_recovery_matches_only_explicit_responses_errors() {
    assert!(responses_tool_call_is_missing_output(
        br#"{"error":{"message":"No tool output found for custom tool call ctc_123"}}"#
    ));
    assert!(responses_tool_call_is_missing_output(
        br#"{"error":{"code":"unanswered_function_call"}}"#
    ));
    assert!(!responses_tool_call_is_missing_output(
        br#"{"error":{"message":"No tool call found for custom tool call output"}}"#
    ));
}

#[test]
fn model_switch_recovery_resets_only_safe_responses_continuations() {
    let mismatch = br#"{"error":{"message":"Tool call output does not match the model that created the previous response"}}"#;
    assert!(recoverable_response_model_switch(
        StatusCode::BAD_REQUEST,
        "upstream_tool_call_mismatch",
        true,
        false,
        mismatch,
    ));
    assert!(recoverable_response_model_switch(
        StatusCode::BAD_REQUEST,
        "upstream_invalid_request",
        true,
        false,
        br#"{"error":{"message":"previous_response_id belongs to another model"}}"#,
    ));
    assert!(!recoverable_response_model_switch(
        StatusCode::BAD_REQUEST,
        "upstream_tool_call_mismatch",
        true,
        true,
        mismatch,
    ));
    assert!(!recoverable_response_model_switch(
        StatusCode::BAD_REQUEST,
        "upstream_invalid_request",
        false,
        false,
        mismatch,
    ));
}

#[test]
fn prompt_cache_write_rejection_requires_explicit_cache_creation_wording() {
    assert!(prompt_cache_write_rejected(
        br#"{"error":{"message":"cache_control ephemeral cache write is unsupported"}}"#
    ));
    assert!(prompt_cache_write_rejected(
        br#"{"error":{"message":"cache creation TTL is invalid"}}"#
    ));
    assert!(!prompt_cache_write_rejected(
        br#"{"error":{"message":"cached input is not available"}}"#
    ));
}
