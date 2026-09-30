use super::*;

#[tokio::test]
async fn non_stream_response_and_usage_are_redacted() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "private prompt",
            "stream": false
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], "resp_test");
    assert_eq!(body["usage"]["total_tokens"], 7);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].input_tokens, Some(3));
    assert_eq!(events[0].output_tokens, Some(4));
    assert_eq!(events[0].total_tokens, Some(7));
    let serialized = serde_json::to_string(&events[0]).unwrap();
    assert!(!serialized.contains("private prompt"));
    assert!(!serialized.contains(LOCAL_KEY));
    assert!(!serialized.contains(SOURCE_KEY));
}

#[tokio::test]
async fn large_client_requests_are_forwarded_with_a_bounded_limit() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "x".repeat(17 * 1024 * 1024),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state.requests.lock().unwrap().len(), 1);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "x".repeat(MAX_CLIENT_REQUEST_BODY_BYTES),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "request_too_large");
    assert_eq!(state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn oversized_non_stream_response_is_rejected_and_recorded() {
    let upstream = spawn(Router::new().route(
        "/v1/responses",
        post(|| async {
            let chunks = stream::iter([
                Ok::<_, Infallible>(Bytes::from(vec![b'x'; 8 * 1024 * 1024])),
                Ok::<_, Infallible>(Bytes::from(vec![b'x'; 8 * 1024 * 1024 + 1])),
            ]);
            Response::builder()
                .status(StatusCode::OK)
                .body(Body::from_stream(chunks))
                .unwrap()
        }),
    ))
    .await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "gpt-test", "input": "private prompt"}))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "upstream_error");
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_body_too_large")
    );
}

#[tokio::test]
async fn stream_prelude_is_buffered_until_the_first_text_output() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let request_url = format!("{}/v1/responses", gateway.base_url);
    let response_task = tokio::spawn(async move {
        reqwest::Client::new()
            .post(request_url)
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model": "gpt-test", "input": "hello", "stream": true}))
            .send()
            .await
            .unwrap()
    });
    state.release_stream.notify_one();
    let response = tokio::time::timeout(Duration::from_secs(1), response_task)
        .await
        .expect("the first native SSE output should establish the stream")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let mut chunks = response.bytes_stream();
    let first = tokio::time::timeout(Duration::from_secs(1), chunks.next())
        .await
        .expect("buffered native SSE frames were not forwarded")
        .unwrap()
        .unwrap();
    let first = std::str::from_utf8(&first).unwrap();
    assert!(first.contains("data: {\"type\":\"response.created\"}\n\n"));
    assert!(
        first.contains("data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello\"}\n\n")
    );
    let second = tokio::time::timeout(Duration::from_secs(1), chunks.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(second, "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"status\":\"completed\"}}\n\n");
    assert!(chunks.next().await.is_none());

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer upstream-test-key")
    );
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert!(events[0]
        .ttft_ms
        .is_some_and(|ttft| ttft <= events[0].latency_ms));
}

#[tokio::test]
async fn fragmented_terminal_sse_records_usage_before_client_disconnect() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "terminal-fragmented",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    let mut chunks = response.bytes_stream();
    let mut received = Vec::new();
    while !received.windows(2).any(|window| window == b"\n\n") {
        let chunk = tokio::time::timeout(Duration::from_secs(1), chunks.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        received.extend_from_slice(&chunk);
    }
    drop(chunks);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if !events.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].input_tokens, Some(2));
    assert_eq!(events[0].output_tokens, Some(3));
    assert_eq!(events[0].total_tokens, Some(5));
    assert_ne!(
        events[0].error_category.as_deref(),
        Some("client_cancelled")
    );
}

#[tokio::test]
async fn coalesced_terminal_sse_does_not_forward_later_output_or_record_it_twice() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    for input in ["coalesced-terminal", "coalesced-terminal-after-output"] {
        let response = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model": "gpt-test", "input": input, "stream": true}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = tokio::time::timeout(Duration::from_secs(2), response.text())
            .await
            .unwrap()
            .unwrap();
        let frames = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .map(|data| serde_json::from_str::<Value>(data).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            frames
                .iter()
                .map(|frame| frame["type"].as_str().unwrap())
                .collect::<Vec<_>>(),
            [
                "response.created",
                "response.output_text.delta",
                "response.completed"
            ],
            "{input}"
        );
        assert_eq!(frames[1]["delta"], "once", "{input}");
    }
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events
        .iter()
        .all(|event| event.success && event.total_tokens == Some(3)));
}

#[tokio::test]
async fn failed_terminal_sse_is_not_recorded_as_success() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "terminal-failed",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    let _ = response.bytes().await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_terminal")
    );
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
}

#[tokio::test]
async fn responses_route_to_chat_completions_sources() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let runtime = GatewayRuntime::new(
        ProviderSource {
            id: "chat-source".to_string(),
            name: "Stateless chat source".to_string(),
            base_url: "http://127.0.0.1:9/v1".to_string(),
            api_key: SOURCE_KEY.to_string(),
            wire_api: WireApi::ChatCompletions,
            models: vec!["gpt-test".to_string()],
        },
        LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        },
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    let gateway = spawn(gateway::router(Arc::new(runtime))).await;
    let client = reqwest::Client::new();

    let response = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "inspect",
            "tools": [{"type": "function", "name": "shell", "parameters": {"type": "object"}}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    for (index, event) in events.iter().enumerate() {
        assert!(!event.success);
        assert_eq!(event.wire_api, WireApi::Responses);
        assert_eq!(
            event.error_category.as_deref(),
            Some("upstream_transport_connect")
        );
        assert_eq!(usize::from(event.attempt), index + 1);
        assert_eq!(event.request_id, events[0].request_id);
    }
}

#[tokio::test]
async fn non_account_compact_summarizes_every_bridged_model() {
    let seen = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let recorded = seen.clone();
    let upstream = spawn(
        Router::new()
            .route(
                "/v1/chat/completions",
                post(move |body: Bytes| {
                    let recorded = recorded.clone();
                    async move {
                        recorded.lock().unwrap().push(body.to_vec());
                        Json(json!({
                            "id": "chatcmpl_compact",
                            "choices": [{
                                "index": 0,
                                "finish_reason": "stop",
                                "message": {"role": "assistant", "content": "Goal: keep src/main.rs"}
                            }]
                        }))
                    }
                }),
            )
            .layer(DefaultBodyLimit::max(MAX_CLIENT_REQUEST_BODY_BYTES)),
    )
    .await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let usage_events = events.clone();
    let runtime = GatewayRuntime::new(
        ProviderSource {
            id: "chat-source".to_string(),
            name: "Stateless chat source".to_string(),
            base_url: format!("{}/v1", upstream.base_url),
            api_key: SOURCE_KEY.to_string(),
            wire_api: WireApi::ChatCompletions,
            models: vec!["vendor-model".to_string()],
        },
        LocalGatewayKey {
            id: "local-key-1".to_string(),
            secret: LOCAL_KEY.to_string(),
        },
        Arc::new(move |event| usage_events.lock().unwrap().push(event)),
    )
    .unwrap();
    let gateway = spawn(gateway::router(Arc::new(runtime))).await;
    let client = reqwest::Client::new();
    let response = client
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "vendor-model",
            "input": "Keep src/main.rs",
            "tools": [{"type": "custom", "name": "apply_patch", "format": {"type": "text"}}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let compact: Value = response.json().await.unwrap();
    assert_eq!(compact["object"], "response.compaction");
    assert_eq!(compact["output"][0]["type"], "compaction");
    assert!(compact["output"][0]["encrypted_content"]
        .as_str()
        .unwrap()
        .starts_with("zenith-relay-compact-v1:"));

    let continued = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "vendor-model",
            "input": [compact["output"][0].clone(), {"type": "message", "role": "user", "content": "continue"}]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(continued.status(), StatusCode::OK);
    let bodies = seen.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    let first = String::from_utf8(bodies[0].clone()).unwrap();
    assert!(first.contains("Keep src/main.rs"));
    assert!(first.contains("Reply with only the summary"));
    assert!(!first.contains("apply_patch"));
    let second = String::from_utf8(bodies[1].clone()).unwrap();
    assert!(second.contains("Goal: keep src/main.rs"));
    assert!(second.contains("continue"));
    assert!(!second.contains("zenith-relay-compact-v1:"));
    assert!(events.lock().unwrap().iter().all(|event| event.success));
}

#[tokio::test]
async fn truncated_prelude_stream_returns_one_terminal_error_and_is_recorded_as_incomplete() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
                "model": "gpt-test",
                "input": "truncated-stream",
                "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "stream_incomplete");

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("stream_incomplete")
    );
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
}

#[tokio::test]
async fn non_success_stream_done_does_not_override_upstream_status() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "input": "limited-stream",
            "stream": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let _ = response.bytes().await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    for (index, event) in events.iter().enumerate() {
        assert!(!event.success);
        assert_eq!(event.http_status, StatusCode::TOO_MANY_REQUESTS.as_u16());
        assert_eq!(usize::from(event.attempt), index + 1);
        assert_eq!(event.request_id, events[0].request_id);
    }
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_rate_limited")
    );
}
