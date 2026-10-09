use super::*;

#[tokio::test]
async fn automatic_adapter_candidate_shares_the_execution_budget() {
    let (chat_server, chat_state) = spawn_upstream("chat-key", Vec::new()).await;
    let (responses_server, responses_state) = spawn_upstream(
        "responses-key",
        vec![response_reply("responses-wins", "winner")],
    )
    .await;
    let chat = source_with_protocol(
        "chat",
        &chat_server,
        "chat-key",
        &[MODEL],
        10,
        WireApi::ChatCompletions,
    );
    let (gateway, events) = spawn_gateway(
        vec![
            chat,
            source("responses", &responses_server, "responses-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        1,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "hello"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let chat_requests = chat_state.requests.lock().unwrap();
    assert_eq!(chat_requests.len(), 1);
    assert_eq!(chat_requests[0].path, "/v1/chat/completions");
    drop(chat_requests);
    assert!(responses_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].attempt, 1);
    assert_eq!(events[0].source_id, "chat");
    assert!(!events[0].success);
}

#[tokio::test]
async fn truncated_prelude_transport_failure_does_not_replay_unknown_work() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response.created\""),
                StreamChunk::Error,
            ],
            cache_control: "loser",
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response."),
                StreamChunk::Data("created\"}\n\n"),
                StreamChunk::Data("data: [DONE]\n\n"),
            ],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.text().await.unwrap();
    assert!(!body.contains("response.created"));
    assert!(!body.contains("response.failed"));
    assert!(!body.contains("[DONE]"));
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_transport")
    );
}

#[tokio::test]
async fn streaming_usage_limit_keeps_the_provider_reset_before_fallback() {
    let (source_a, _) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data(
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"usage_limit_reached\",\"resets_in_seconds\":120}}}\n\n",
            )],
            cache_control: "limited",
        }],
    )
    .await;
    let (source_b, _) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response.created\"}\n\n"),
                StreamChunk::Data("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_winner\",\"status\":\"completed\"}}\n\n"),
            ],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "winner");
    let _ = response.text().await.unwrap();

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let events = events.lock().unwrap();
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_quota_exhausted")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some("*"));
    assert!(events[0]
        .retry_at_ms
        .is_some_and(|retry_at| retry_at > now_ms + 100_000));
    assert!(events[1].success);
}

#[tokio::test]
async fn streaming_plan_entitlement_failure_falls_back_without_blocking_the_account() {
    let (source_a, _) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data(
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"usage_not_included\"}}}\n\n",
            )],
            cache_control: "limited",
        }],
    )
    .await;
    let (source_b, _) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_winner\",\"status\":\"completed\"}}\n\n")],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "winner");
    let _ = response.text().await.unwrap();

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].http_status, StatusCode::FORBIDDEN.as_u16());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_usage_not_included")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some("*"));
    assert!(events[1].success);
}

#[tokio::test]
async fn streaming_generic_gateway_rejection_does_not_try_fallback() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data(
                "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"bad_request\",\"message\":\"Zenith AI request is invalid. Check the model, messages, tools, and parameters.\"}}}\n\n",
            )],
            cache_control: "rejected",
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_fallback\",\"status\":\"completed\",\"output\":[]}}\n\n")],
            cache_control: "fallback",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = response.text().await.unwrap();
    assert!(body.contains("Zenith AI request is invalid"), "body={body}");
    assert!(!body.contains("resp_fallback"), "body={body}");
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_invalid_request")
    );
    assert!(events[0].cooldown_scope.is_none());
}

#[tokio::test]
async fn bridged_messages_prelude_returns_one_safe_terminal_error() {
    let (upstream, state) = spawn_upstream(
        "messages-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data(
                    "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_gateway_error\",\"usage\":{\"input_tokens\":1}}}\n\n",
                ),
                StreamChunk::Data(
                    "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"code\":\"service_unavailable\",\"message\":\"Zenith AI service is temporarily unavailable. Please retry later.\"}}\n\n",
                ),
            ],
            cache_control: "messages",
        }],
    )
    .await;
    let messages = source_with_protocol(
        "messages",
        &upstream,
        "messages-key",
        &[MODEL],
        0,
        WireApi::Messages,
    );
    let (gateway, events) =
        spawn_gateway(vec![messages], vec![local_key("key", LOCAL_KEY, None)], 3).await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = response.text().await.unwrap();
    assert!(!body.contains("response.created"), "body={body}");
    assert!(body.contains("service_unavailable"), "body={body}");
    assert!(
        body.contains("Zenith AI service is temporarily unavailable. Please retry later."),
        "body={body}"
    );
    assert!(
        !body.contains("adapter_upstream_stream_invalid"),
        "body={body}"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 1);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_unavailable")
    );
}

#[tokio::test]
async fn complete_prelude_transport_failure_does_not_replay_unknown_work() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response.created\"}\n\n"),
                StreamChunk::Error,
            ],
            cache_control: "first",
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data("data: [DONE]\n\n")],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.text().await.unwrap();
    assert!(!body.contains("response.created"));
    assert!(!body.contains("response.failed"));
    assert!(!body.contains("[DONE]"));
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_transport")
    );
}

#[tokio::test]
async fn invalid_sse_after_a_native_prelude_does_not_replay_unknown_work() {
    let (source_a, state_a) = spawn_upstream(
        "a-key",
        vec![Reply::Stream {
            chunks: vec![
                StreamChunk::Data("data: {\"type\":\"response.created\"}\n\n"),
                StreamChunk::Data("data: {not-"),
                StreamChunk::Data("json}\n\n"),
                StreamChunk::Data("data: [DONE]\n\n"),
            ],
            cache_control: "first",
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "b-key",
        vec![Reply::Stream {
            chunks: vec![StreamChunk::Data("data: [DONE]\n\n")],
            cache_control: "winner",
        }],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 10),
            source("b", &source_b, "b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response.text().await.unwrap();
    assert!(!body.contains("response.created"));
    assert!(!body.contains("data: {not-"));
    assert!(!body.contains("response.failed"));
    assert!(!body.contains("[DONE]"));
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].error_category.as_deref(), Some("stream_invalid"));
}
