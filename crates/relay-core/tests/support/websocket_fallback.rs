use super::*;

#[tokio::test]
async fn websocket_http_fallback_preserves_json_compaction_events() {
    let (upstream, state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data(
            "data: {\"type\":\"response.created\",\"response\":{\"id\":\"compact-setup\"}}\n\n",
        ),
        StreamChunk::Data(
            "event: response.compaction.delta\ndata: {  \"type\": \"response.compaction.delta\", \"opaque\": true }\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"compact-done\"}}\n\n",
        ),
    ])])
    .await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source("compact-source", &upstream, "source-key", 10)],
        Vec::new(),
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "compact",
                "context_management": [{"type": "compaction", "compact_threshold": 1000}]
            })
            .to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.created"
    );
    let compaction = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match compaction {
        ClientWsMessage::Text(text) => assert_eq!(
            text,
            "{  \"type\": \"response.compaction.delta\", \"opaque\": true }"
        ),
        _ => panic!("compaction event must remain a text WebSocket message"),
    }
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "compact-done"
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].body["context_management"][0]["compact_threshold"],
        1000
    );
    assert_eq!(requests[0].body["stream"], true);
}

#[tokio::test]
async fn websocket_http_fallback_accepts_compaction_events_over_one_mebibyte() {
    let large_data = "x".repeat(1024 * 1024 + 128);
    let large_event = Box::leak(
        format!(
            "event: response.compaction.delta\ndata: {{\"type\":\"response.compaction.delta\",\"opaque\":\"{large_data}\"}}\n\n"
        )
        .into_boxed_str(),
    );
    let (upstream, _) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data(large_event),
        StreamChunk::Data(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"large-compact\"}}\n\n",
        ),
    ])])
    .await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source("large-compact-source", &upstream, "source-key", 10)],
        Vec::new(),
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "large compact"})
                .to_string(),
        ))
        .await
        .unwrap();

    let compact = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match compact {
        ClientWsMessage::Text(text) => {
            assert!(text.len() > 1024 * 1024);
            assert!(text.starts_with("{\"type\":\"response.compaction.delta\""));
        }
        _ => panic!("large compaction event must remain a text WebSocket message"),
    }
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "large-compact"
    );
}

#[tokio::test]
async fn managed_websocket_quota_failure_falls_back_to_http_api_source() {
    let reset_at = current_time_ms() / 1_000 + 6 * 24 * 60 * 60;
    let (account_upstream, account_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type": "response.failed",
            "response": {
                "error": {
                    "type": "usage_limit_reached",
                    "message": "Usage limit reached. Try again after the reset.",
                    "resets_at": reset_at
                }
            }
        })])))
        .await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![Reply::Stream(vec![StreamChunk::Data(
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"api fallback\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"http-fallback\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        )])])
        .await;
    let authority = ready_authority("limited-account", "limited-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("api-source", &source_upstream, "source-key", -100)],
        vec![account(
            "limited-account",
            "provider-limited",
            &account_upstream,
            100,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    gateway
        .runtime
        .as_ref()
        .unwrap()
        .set_route_recovery_enabled(true);

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("originator", "codex_cli_rs")
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "use the pool"}).to_string(),
        ))
        .await
        .unwrap();

    let first = receive_websocket_json(&mut socket).await;
    assert_eq!(first["type"], "response.output_text.delta");
    assert_eq!(first["delta"], "api fallback");
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "http-fallback"
    );
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(account_state.requests.lock().unwrap().len(), 1);
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_quota_exhausted")
    );
    assert!(events[1].success);
    assert_eq!(events[1].candidate_id.as_deref(), Some("api-source"));
}

#[tokio::test]
async fn account_websocket_does_not_retry_after_output_begins() {
    let (failing_upstream, failing_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type": "response.output_text.delta", "delta": "partial"}),
            json!({
                "type": "error",
                "status": 502,
                "error": {"message": "synthetic late failure"}
            }),
        ])))
        .await;
    let (reserve_upstream, reserve_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "failing-account", "failing-access").await;
    register_ready(&authority, "reserve-account", "reserve-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "failing-account",
                "provider-failing",
                &failing_upstream,
                200,
            ),
            account(
                "reserve-account",
                "provider-reserve",
                &reserve_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "do not replay"})
                .to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(failing_state.requests.lock().unwrap().len(), 1);
    assert!(reserve_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
}

#[tokio::test]
async fn account_websocket_closes_after_a_late_close_without_replaying() {
    let (failing_upstream, failing_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::OutputThenClose).await;
    let (reserve_upstream, reserve_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "failing-account", "failing-access").await;
    register_ready(&authority, "reserve-account", "reserve-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "failing-account",
                "provider-failing",
                &failing_upstream,
                200,
            ),
            account(
                "reserve-account",
                "provider-reserve",
                &reserve_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "close late"}).to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    let close = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        close,
        ClientWsMessage::Close { code: _, reason: _ }
    ));
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(failing_state.requests.lock().unwrap().len(), 1);
    assert!(reserve_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].http_status, StatusCode::BAD_GATEWAY.as_u16());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_websocket_closed")
    );
}

#[tokio::test]
async fn abrupt_websocket_disconnect_releases_its_lease_without_retrying() {
    let release = Arc::new(Notify::new());
    let (upstream, state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Hold(release.clone())).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "cancel me"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.output_text.delta"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        1
    );
    drop(socket);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let idle =
                gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight == 0;
            if idle && !events.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("disconnected websocket lease was not released");
    release.notify_waiters();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("client_websocket")
    );
    assert_eq!(events[0].retry_at_ms, None);
}

#[tokio::test]
async fn account_websocket_reselects_for_each_independent_request() {
    let (first_upstream, first_state) = spawn_websocket_upstream().await;
    let (second_upstream, second_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &first_upstream, 100),
            account("second-account", "provider-second", &second_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    for input in ["first independent request", "second independent request"] {
        socket
            .send(ClientWsMessage::Text(
                json!({"type": "response.create", "model": MODEL, "input": input}).to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.output_text.delta"
        );
        assert_eq!(
            receive_websocket_completion(&mut socket).await["type"],
            "response.completed"
        );
    }
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(first_state.requests.lock().unwrap().len(), 1);
    assert_eq!(second_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_ne!(events[0].candidate_id, events[1].candidate_id);
}
