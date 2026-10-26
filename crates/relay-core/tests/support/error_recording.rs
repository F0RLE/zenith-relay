use super::*;

fn missing_adapter_history_request(stream: bool) -> Value {
    json!({
        "model":"claude-test", "stream":stream,
        "input":[
            {"role":"user", "content":"private-preparation-sentinel"},
            {"type":"function_call", "call_id":"call_synthetic", "name":"read_file", "arguments":"{}"},
            {"type":"function_call_output", "call_id":"call_synthetic", "output":"synthetic tool result"}
        ],
        "tools":[{"type":"function", "name":"read_file", "parameters":{"type":"object"}}]
    })
}

#[tokio::test]
async fn adapter_preparation_rejections_are_recorded_without_a_provider_attempt() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Budget)
            .await;
    let client = reqwest::Client::new();
    for stream in [false, true] {
        let response = client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&missing_adapter_history_request(stream))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["error"]["code"], "adapter_continuation_missing");
        assert_eq!(body["error"]["zenith_relay"]["origin"], "relay");
    }
    assert!(state.requests.lock().unwrap().is_empty());
    let recorded = events.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    for event in recorded.iter() {
        assert!(!event.success);
        assert_eq!(event.http_status, 400);
        assert_eq!(
            event.error_category.as_deref(),
            Some("adapter_continuation_missing")
        );
        assert_eq!(event.source_id, "relay");
        assert!(event.candidate_id.is_none());
        assert!(event.account_id.is_none());
        assert!(event.total_tokens.is_none());
        assert!(event.input_tokens.is_none());
        assert!(event.output_tokens.is_none());
        assert!(event.cooldown_scope.is_none());
        assert!(event.consecutive_failures.is_none());
        assert!(
            !serde_json::to_string(event)
                .unwrap()
                .contains("private-preparation-sentinel")
        );
    }
}

#[tokio::test]
async fn websocket_adapter_preparation_rejection_is_recorded_once() {
    let (upstream, state) = spawn_messages_upstream().await;
    let (gateway, events) =
        spawn_messages_bridge_gateway(&upstream.base_url, &state, MessagesReasoningMode::Budget)
            .await;
    let mut socket = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    let mut request = missing_adapter_history_request(true);
    request["type"] = json!("response.create");
    socket
        .send(ClientWsMessage::Text(request.to_string()))
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ClientWsMessage::Text(frame) = frame else {
        panic!("expected a local protocol error");
    };
    let error: Value = serde_json::from_str(&frame).unwrap();
    assert_eq!(error["type"], "error");
    assert_eq!(error["error"]["code"], "previous_response_not_found");
    assert_eq!(error["error"]["param"], "previous_response_id");
    assert_eq!(
        error["error"]["zenith_relay"]["category"],
        "adapter_continuation_missing"
    );
    assert!(state.requests.lock().unwrap().is_empty());
    let recorded = events.lock().unwrap();
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded[0].error_category.as_deref(),
        Some("adapter_continuation_missing")
    );
    assert_eq!(
        recorded[0].transport,
        zenith_relay_core::UsageTransport::Websocket
    );
    assert!(recorded[0].candidate_id.is_none());
    assert!(recorded[0].total_tokens.is_none());
}

#[tokio::test]
async fn original_errors_reach_usage_for_http_failed_json_and_sse() {
    for (status, streaming) in [(422, false), (200, false), (200, true)] {
        let payload = json!({"error": {"code": "future_validation_error", "type": "invalid_request_error", "message": "Invalid field: temperature"}});
        let body = if streaming {
            format!(
                "data: {}\n\n",
                json!({"type": "response.failed", "response": payload})
            )
        } else {
            payload.to_string()
        };
        let upstream = spawn(Router::new().route(
            "/v1/responses",
            post(move || {
                let body = body.clone();
                async move {
                    Response::builder()
                        .status(status)
                        .header(
                            CONTENT_TYPE,
                            if streaming {
                                "text/event-stream"
                            } else {
                                "application/json"
                            },
                        )
                        .body(Body::from(body))
                        .unwrap()
                }
            }),
        ))
        .await;
        let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":"gpt-test", "input":"synthetic input", "stream":streaming}))
            .send()
            .await
            .unwrap();
        let response = response.text().await.unwrap();
        assert!(response.contains("Invalid field: temperature"));
        let events = events.lock().unwrap();
        let events = usage_observations::physical_attempts(&events);
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert!(!event.success);
        assert_eq!(
            event.error_category.as_deref(),
            Some("upstream_invalid_request")
        );
        assert!(event.cooldown_scope.is_none());
        let details = event.upstream_error.as_ref().unwrap();
        assert_eq!(details.http_status, Some(status));
        assert_eq!(details.code.as_deref(), Some("future_validation_error"));
        assert_eq!(
            details.message.as_deref(),
            Some("Invalid field: temperature")
        );
    }
}

#[tokio::test]
async fn malformed_sse_records_parser_diagnostics_before_output() {
    let upstream = spawn(Router::new().route(
        "/v1/responses",
        post(|| async {
            Response::builder()
                .header(CONTENT_TYPE, "text/event-stream")
                .body(Body::from(
                    "event: synthetic-private\ndata: {\"synthetic-private\":\n\n",
                ))
                .unwrap()
        }),
    ))
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":"gpt-test", "input":"synthetic input", "stream":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let response: Value = response.json().await.unwrap();
    assert_eq!(response["error"]["code"], "stream_invalid");
    let events = events.lock().unwrap();
    assert!(!events.is_empty());
    for event in events.iter() {
        assert!(!event.success);
        assert_eq!(event.error_category.as_deref(), Some("stream_invalid"));
        assert!(event.ttft_ms.is_none());
        let details = event.upstream_error.as_ref().unwrap();
        assert_eq!(details.http_status, Some(200));
        assert_eq!(details.error_type.as_deref(), Some("relay_stream_parser"));
        let message = details.message.as_deref().unwrap();
        assert!(message.contains("category=Eof"));
        assert!(!message.contains("synthetic-private"));
    }
}
