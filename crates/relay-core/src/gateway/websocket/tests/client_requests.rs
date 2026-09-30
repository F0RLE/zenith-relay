use super::*;
use serde_json::json;

#[test]
fn client_request_resolves_the_visible_model_before_upstream_serialization() {
    let runtime = runtime();
    runtime.bind_response_affinity(Some("resp_previous"), "source", crate::unix_time_ms());
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        crate::gateway::request::CODEX_RESPONSES_LITE_HEADER,
        HeaderValue::from_static("true"),
    );
    let request_result = ClientRequest::parse(
        &runtime,
        &key,
        &headers,
        br#"{
            "model": "relay/upstream-model",
            "input": "hello",
            "previous_response_id": "resp_previous",
            "prompt_cache_key": "cache-key"
        }"#,
    );
    assert!(request_result.is_ok(), "request should be accepted");
    let Ok(request) = request_result else {
        return;
    };
    let route = runtime
        .executor_route(
            "source",
            &request.resolved_model,
            &key.scope_snapshot(),
            WEBSOCKET_PROTOCOLS,
            false,
        )
        .unwrap();
    let payload_result = request.payload_for(&route);
    assert!(payload_result.is_ok(), "request should be serializable");
    let Ok(payload_text) = payload_result else {
        return;
    };
    let payload: serde_json::Value = serde_json::from_str(&payload_text).unwrap();

    assert_eq!(request.requested_model, "relay/upstream-model");
    assert_eq!(request.resolved_model, "upstream-model");
    assert!(request.responses_lite);
    assert!(request.response_affinity_key.is_some());
    assert!(request.requires_affinity_owner);
    assert!(request.prompt_affinity_key.is_some());
    assert_eq!(payload["type"], "response.create");
    assert_eq!(payload["model"], "upstream-model");
    assert_eq!(payload["input"], "hello");
}

#[test]
fn client_request_rejects_unknown_previous_response_even_with_plaintext_history() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let request = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        br#"{
            "type": "response.create",
            "model": "relay/upstream-model",
            "previous_response_id": "resp_external_history",
            "input": [
                {"type":"message","role":"user","content":"What is the capital of France?"},
                {"type":"message","role":"assistant","content":"Paris is the capital of France."},
                {"type":"message","role":"user","content":"Name one landmark there."}
            ]
        }"#,
    )
    .err()
    .expect("client message shape cannot prove complete history");

    assert_eq!(request.status, StatusCode::CONFLICT);
}

#[test]
fn client_request_rejects_an_unknown_opaque_previous_response_before_selection() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let error = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        br#"{
            "type": "response.create",
            "model": "relay/upstream-model",
            "previous_response_id": "resp_external_opaque",
            "input": "continue"
        }"#,
    )
    .err()
    .expect("unknown opaque continuation must not reach selection");

    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.category, "response_continuation_unavailable");
}

#[test]
fn client_request_rejects_an_unknown_tool_continuation_before_selection() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let error = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        br#"{
            "type": "response.create",
            "model": "relay/upstream-model",
            "previous_response_id": "resp_external_tool",
            "input": [{"type":"function_call_output","call_id":"call_external","output":"done"}]
        }"#,
    )
    .err()
    .expect("unknown tool continuation must not reach selection");

    assert_eq!(error.status, StatusCode::CONFLICT);
    assert_eq!(error.category, "response_continuation_unavailable");
}

#[test]
fn websocket_lite_contract_is_normalized_before_non_account_routing() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        crate::gateway::request::CODEX_RESPONSES_LITE_HEADER,
        HeaderValue::from_static("true"),
    );
    let request_result = ClientRequest::parse(
        &runtime,
        &key,
        &headers,
        br#"{
            "type": "response.create",
            "model": "relay/upstream-model",
            "input": "hello",
            "parallel_tool_calls": true
        }"#,
    );
    assert!(request_result.is_ok(), "request should be accepted");
    let Ok(request) = request_result else {
        return;
    };
    let route = runtime
        .executor_route(
            "source",
            &request.resolved_model,
            &key.scope_snapshot(),
            WEBSOCKET_PROTOCOLS,
            false,
        )
        .expect("test source should be routable");
    let payload_result = request.payload_for(&route);
    assert!(payload_result.is_ok(), "request should serialize");
    let Ok(payload_bytes) = payload_result else {
        return;
    };
    let payload: serde_json::Value =
        serde_json::from_str(&payload_bytes).expect("payload should be valid JSON");

    assert_eq!(payload["parallel_tool_calls"], false);
    assert!(request.responses_lite_for(&route));
}

#[test]
fn websocket_lite_contract_rejects_non_boolean_parallel_tools() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        crate::gateway::request::CODEX_RESPONSES_LITE_HEADER,
        HeaderValue::from_static("true"),
    );
    let parse_result = ClientRequest::parse(
        &runtime,
        &key,
        &headers,
        br#"{
            "type": "response.create",
            "model": "relay/upstream-model",
            "input": "hello",
            "parallel_tool_calls": "yes"
        }"#,
    );
    assert!(
        parse_result.is_err(),
        "non-boolean Lite tool setting must be rejected"
    );
    let error = match parse_result {
        Err(error) => error,
        Ok(_) => return,
    };

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        error.message,
        "responses Lite requires parallel_tool_calls to be a boolean"
    );
}

#[test]
fn client_request_rejects_non_create_messages_before_candidate_selection() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let error = match ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        br#"{"type":"response.cancel","model":"relay/upstream-model"}"#,
    ) {
        Ok(_) => panic!("non-create message should be rejected"),
        Err(error) => error,
    };

    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert_eq!(error.category, "invalid_request");
    assert_eq!(error.message, "only response.create messages are supported");
}

#[test]
fn client_request_accepts_and_preserves_a_stream_id() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let request = match ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        br#"{"type":"response.create","stream_id":"main","model":"relay/upstream-model","input":"hello"}"#,
    ) {
        Ok(request) => request,
        Err(error) => panic!("stream_id should be accepted: {}", error.message),
    };
    assert_eq!(request.stream_id.as_deref(), Some("main"));
    let route = runtime
        .executor_route(
            "source",
            &request.resolved_model,
            &key.scope_snapshot(),
            WEBSOCKET_PROTOCOLS,
            false,
        )
        .expect("test source should be routable");
    let payload = request
        .payload_for(&route)
        .unwrap_or_else(|error| panic!("payload should serialize: {}", error.message));
    assert!(payload.contains("\"stream_id\":\"main\""));
}

#[test]
fn client_request_rejects_invalid_named_stream_ids() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    for invalid in [
        "",
        " main",
        "main ",
        "other/lane",
        "a b",
        "café",
        "a\nb",
        "a".repeat(257).as_str(),
    ] {
        let payload = serde_json::to_vec(&json!({
            "type": "response.create",
            "stream_id": invalid,
            "model": "relay/upstream-model",
            "input": "hello",
        }))
        .unwrap();
        let failure = ClientRequest::parse(&runtime, &key, &HeaderMap::new(), &payload)
            .err()
            .expect("invalid named stream_id must be rejected");
        assert_eq!(failure.status, StatusCode::BAD_REQUEST);
        assert_eq!(failure.category, "invalid_stream_id");
    }
}

#[test]
fn websocket_request_can_repair_foreign_message_item_ids() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let mut request = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        br#"{
            "type": "response.create",
            "model": "relay/upstream-model",
            "input": [
                {"type":"message","id":"item_foreign","role":"assistant","content":"hello"},
                {"type":"message","id":"msg_native","role":"user","content":"continue"},
                {"type":"reasoning","id":"item_reasoning","summary":[]}
            ]
        }"#,
    )
    .unwrap_or_else(|error| panic!("request should be accepted: {}", error.message));

    assert!(request.repair_message_item_ids());
    let route = runtime
        .executor_route(
            "source",
            &request.resolved_model,
            &key.scope_snapshot(),
            WEBSOCKET_PROTOCOLS,
            false,
        )
        .expect("test source should be routable");
    let payload: serde_json::Value = serde_json::from_str(
        &request
            .payload_for(&route)
            .unwrap_or_else(|error| panic!("request should serialize: {}", error.message)),
    )
    .expect("payload should be valid JSON");

    assert!(payload.pointer("/input/0/id").is_none());
    assert_eq!(payload.pointer("/input/1/id"), Some(&json!("msg_native")));
    assert_eq!(
        payload.pointer("/input/2/id"),
        Some(&json!("item_reasoning"))
    );
    assert!(!request.repair_message_item_ids());
}

#[test]
fn websocket_request_repairs_legacy_call_ids_and_recomputes_affinity() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let request_result = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        br#"{
            "type": "response.create",
            "model": "relay/upstream-model",
            "input": [
                {"type":"function_call","name":"lookup","arguments":"{}"},
                {"type":"function_call_output","output":"result"}
            ]
        }"#,
    );
    assert!(request_result.is_ok(), "request should be accepted");
    let Ok(mut request) = request_result else {
        return;
    };

    assert!(!request.has_unpaired_tool_output());
    assert!(request.repair_legacy_call_ids());
    assert!(!request.has_unpaired_tool_output());
    assert!(!request.requires_affinity_owner);

    let route = runtime
        .executor_route(
            "source",
            &request.resolved_model,
            &key.scope_snapshot(),
            WEBSOCKET_PROTOCOLS,
            false,
        )
        .expect("test source should be routable");
    let payload_result = request.payload_for(&route);
    assert!(payload_result.is_ok(), "request should serialize");
    let Ok(payload_bytes) = payload_result else {
        return;
    };
    let payload: serde_json::Value =
        serde_json::from_str(&payload_bytes).expect("payload should be valid JSON");
    let input = payload["input"].as_array().expect("input array");
    assert_eq!(input.len(), 2);
    assert_eq!(input[0]["type"], "function_call");
    assert_eq!(input[0]["call_id"], input[1]["call_id"]);
    assert!(!request.repair_legacy_call_ids());
}
