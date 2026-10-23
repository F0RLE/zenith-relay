use super::*;
use serde_json::json;

#[tokio::test]
async fn prepared_websocket_diagnostics_match_the_exact_sent_body() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let body = json!({
        "type":"response.create",
        "model":"relay/upstream-model",
        "input":[{"role":"user","content":"synthetic input\n".repeat(65536)}],
        "reasoning":{"effort":"high"},
        "tools":[{"type":"function","name":"fixture","parameters":{"type":"object"}}]
    });
    let request = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        &serde_json::to_vec(&body).unwrap(),
    )
    .unwrap_or_else(|error| panic!("request should parse: {}", error.message));
    let mut route = runtime
        .executor_route(
            "source",
            &request.resolved_model,
            &key.scope_snapshot(),
            WEBSOCKET_PROTOCOLS,
            false,
        )
        .unwrap();
    let expected_payload = request
        .payload_for(&route)
        .unwrap_or_else(|error| panic!("payload should serialize: {}", error.message));
    let expected_reasoning = request.reasoning_effort_for(&route);
    let expected_tools = request.tool_use_for(&route);
    let authorization = runtime
        .prepare_route_authorization(&route, crate::unix_time_ms())
        .await
        .unwrap();
    let prepared = request
        .observed_payload_for(&runtime, &mut route, &HeaderMap::new(), &authorization)
        .unwrap_or_else(|error| panic!("payload should prepare: {}", error.message));
    assert_eq!(prepared.payload, expected_payload);
    assert_eq!(prepared.reasoning_effort, expected_reasoning);
    assert_eq!(
        prepared.tool_use.forwarded_tool_count,
        expected_tools.forwarded_tool_count
    );
    assert_eq!(
        prepared.tool_use.forwarded_schema_bytes,
        expected_tools.forwarded_schema_bytes
    );
    let sent: Value = serde_json::from_str(&prepared.payload).unwrap();
    assert_eq!(
        prepared.tool_use.upstream_context,
        Some(crate::usage::RequestContextSize::capture(&sent))
    );
    assert_eq!(sent["input"], body["input"]);
    assert_eq!(sent["reasoning"], body["reasoning"]);
}

#[tokio::test]
async fn websocket_continuation_accepts_paired_history_from_an_earlier_route() {
    let runtime = GatewayRuntime::from_pool(
        ["earlier", "current"]
            .map(|id| {
                RuntimeSource::unrestricted(ProviderSource {
                    id: id.into(),
                    name: id.into(),
                    base_url: "https://example.test/v1".into(),
                    api_key: "synthetic-secret".into(),
                    wire_api: WireApi::Responses,
                    models: vec!["upstream-model".into()],
                    proxy_url: None,
                })
            })
            .to_vec(),
        vec![RuntimeLocalKey::unrestricted(crate::LocalGatewayKey {
            id: "key".into(),
            secret: "local-secret".into(),
        })],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let route = |candidate| {
        runtime
            .executor_route(
                candidate,
                "upstream-model",
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .unwrap()
    };
    let earlier = route("earlier");
    let mut current = route("current");
    let now = crate::unix_time_ms();
    runtime.bind_tool_call_route_affinity("key", "call_earlier", &earlier, now);
    runtime.bind_response_route_affinity(Some("resp_http"), &current, now);
    let body = json!({
        "type": "response.create",
        "model": "upstream-model",
        "previous_response_id": "resp_http",
        "input": [
            {"type": "function_call", "call_id": "call_earlier", "name": "lookup", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "call_earlier", "output": "synthetic result"},
            {"role": "user", "content": "continue"}
        ]
    });
    let request = ClientRequest::parse(
        &runtime,
        &key,
        &HeaderMap::new(),
        &serde_json::to_vec(&body).unwrap(),
    )
    .unwrap_or_else(|error| panic!("request should parse: {}", error.message));
    let (_, admitted) = request
        .compatible_routes(&runtime, &key)
        .unwrap_or_else(|error| panic!("request should be routable: {}", error.message));
    assert!(
        admitted.contains_key("current"),
        "the predecessor's owner must accept complete tool history"
    );
    assert!(
        !admitted.contains_key("earlier"),
        "the opaque predecessor still pins its own route"
    );
    current.client_transport = crate::UsageTransport::Websocket;
    let prepared = runtime
        .prepare_route_authorization(&current, now)
        .await
        .unwrap();
    let payload: Value = serde_json::from_str(
        &request
            .observed_payload_for(&runtime, &mut current, &HeaderMap::new(), &prepared)
            .unwrap_or_else(|error| panic!("continuation should serialize: {}", error.message))
            .payload,
    )
    .unwrap();
    assert_eq!(
        payload["previous_response_id"],
        body["previous_response_id"]
    );
    assert_eq!(payload["input"], body["input"]);

    let mut orphan = body;
    orphan["input"].as_array_mut().unwrap().remove(0);
    assert_eq!(
        runtime
            .validate_continuation_route(&current, "key", &orphan, now)
            .unwrap_err()
            .code(),
        "adapter_continuation_mismatch"
    );
}

#[test]
fn websocket_frame_identity_keeps_root_affinity_and_child_context_separate() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let parse = |thread, cache_key| {
        let body = serde_json::to_vec(&json!({
            "type": "response.create", "model": "relay/upstream-model", "input": "synthetic",
            "client_metadata": {"session_id": "synthetic-root", "thread_id": thread},
            "prompt_cache_key": cache_key
        }))
        .unwrap();
        ClientRequest::parse(&runtime, &key, &HeaderMap::new(), &body)
            .unwrap_or_else(|_| panic!("synthetic WebSocket request must parse"))
    };
    let first = parse("child-a", None::<&str>);
    let second = parse("child-b", None::<&str>);
    assert!(first.prompt_affinity_key.is_some());
    assert_eq!(first.prompt_affinity_key, second.prompt_affinity_key);
    assert_ne!(first.client_context_id, second.client_context_id);
    assert!(!first.requires_affinity_owner);
    assert!(!second.requires_affinity_owner);
    let cached = parse("child-a", Some("explicit-cache"));
    assert_ne!(first.prompt_affinity_key, cached.prompt_affinity_key);
    assert!(cached
        .prompt_affinity_key
        .as_deref()
        .unwrap()
        .starts_with("cache:"));
}

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
fn websocket_lite_marker_preserves_parallel_policy_on_non_account_routes() {
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

    assert_eq!(payload["parallel_tool_calls"], true);
    assert!(request.responses_lite_for(&route));
    let mut native_route = route.clone();
    native_route.account_id = Some("synthetic-native-account".into());
    native_route.account_transport = crate::runtime::AccountTransport::NativeResponses;
    let error = request.payload_for(&native_route).unwrap_err();
    assert_eq!(error.status, StatusCode::BAD_REQUEST);
    assert!(error
        .message
        .contains("does not support parallel_tool_calls: true"));
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
#[test]
fn account_websocket_cache_key_matches_http_without_changing_api_source_requests() {
    let runtime = runtime();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-codex-session-id",
        HeaderValue::from_static("synthetic-session"),
    );
    for explicit in [None, Some("client-cache-key")] {
        let mut body =
            serde_json::json!({"model":"relay/upstream-model","input":"Synthetic question"});
        if let Some(explicit) = explicit {
            body["prompt_cache_key"] = serde_json::json!(explicit);
        }
        let request = ClientRequest::parse(
            &runtime,
            &key,
            &headers,
            &serde_json::to_vec(&body).unwrap(),
        )
        .unwrap_or_else(|_| panic!("synthetic request must parse"));
        let mut route = runtime
            .executor_route(
                "source",
                &request.resolved_model,
                &key.scope_snapshot(),
                WEBSOCKET_PROTOCOLS,
                false,
            )
            .unwrap();
        let source_payload: Value = serde_json::from_str(
            &request
                .payload_for(&route)
                .unwrap_or_else(|_| panic!("synthetic source request must serialize")),
        )
        .unwrap();
        assert_eq!(
            source_payload.get("prompt_cache_key"),
            body.get("prompt_cache_key")
        );
        route.account_id = Some("synthetic-account".into());
        route.account_transport = crate::runtime::AccountTransport::NativeResponses;
        let account_payload: Value = serde_json::from_str(
            &request
                .payload_for(&route)
                .unwrap_or_else(|_| panic!("synthetic account request must serialize")),
        )
        .unwrap();
        let mut http_fields = body.as_object().unwrap().clone();
        crate::gateway::request::apply_account_prompt_cache_key(
            &mut http_fields,
            &key.id,
            &request.resolved_model,
            request.client_context_id.as_deref(),
        );
        assert_eq!(
            account_payload["prompt_cache_key"],
            http_fields["prompt_cache_key"]
        );
        assert!(account_payload["prompt_cache_key"].as_str().is_some());
        assert_eq!(
            account_payload["input"].as_array().unwrap()[0]["content"][0]["text"],
            "Synthetic question"
        );
        let fallback: Value = serde_json::from_slice(
            &request
                .http_payload()
                .unwrap_or_else(|_| panic!("synthetic fallback must serialize")),
        )
        .unwrap();
        assert_eq!(
            fallback.get("prompt_cache_key"),
            body.get("prompt_cache_key")
        );
        assert_eq!(fallback["input"], body["input"]);
    }
}
