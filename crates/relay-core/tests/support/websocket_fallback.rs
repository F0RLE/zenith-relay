use super::*;

#[tokio::test]
async fn slow_optional_handshake_falls_back_once_without_disabling_native_websocket() {
    check_slow_handshake_fallback(1, &["safe-http-fallback", "native-after-slow"], 2).await;
}

#[tokio::test]
async fn repeated_slow_handshakes_back_off_without_replaying_generations() {
    check_slow_handshake_fallback(usize::MAX, &["safe-http-fallback"; 5], 2).await;
}

async fn check_slow_handshake_fallback(
    slow_connections: usize,
    expected_ids: &[&str],
    expected_connects: usize,
) {
    let connects = Arc::new(AtomicUsize::new(0));
    let http_sends = Arc::new(AtomicUsize::new(0));
    let ws_sends = Arc::new(AtomicUsize::new(0));
    let get_connects = connects.clone();
    let get_sends = ws_sends.clone();
    let post_sends = http_sends.clone();
    let app = Router::new().route("/v1/responses", get(move |upgrade: WebSocketUpgrade| {
        let connects = get_connects.clone();
        let sends = get_sends.clone();
        async move {
            if connects.fetch_add(1, Ordering::SeqCst) < slow_connections {
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            upgrade.on_upgrade(move |mut socket| async move {
                if let Some(Ok(AxumWsMessage::Text(_))) = socket.recv().await {
                    sends.fetch_add(1, Ordering::SeqCst);
                    let _ = socket.send(AxumWsMessage::Text(json!({
                        "type": "response.completed", "response": {"id": "native-after-slow", "status": "completed", "output": []}
                    }).to_string().into())).await;
                }
            })
        }
    }).post(move || {
        post_sends.fetch_add(1, Ordering::SeqCst);
        async {
            Response::builder().header(CONTENT_TYPE, "text/event-stream").body(Body::from(
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"safe-http-fallback\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"
            )).unwrap()
        }
    }));
    let upstream = spawn(app).await;
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
        vec![source("slow-source", &upstream, "synthetic-key", 10)],
        Vec::new(),
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(4).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            diagnostics_enabled: || true,
            ..Default::default()
        },
    )
    .await;
    let mut durations = Vec::new();
    for expected_id in expected_ids {
        let started = std::time::Instant::now();
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
                json!({"type":"response.create", "model":MODEL, "input":"synthetic"}).to_string(),
            ))
            .await
            .unwrap();
        let completion =
            receive_websocket_completion_with_timeout(&mut socket, Duration::from_secs(8)).await;
        durations.push(started.elapsed().as_millis());
        assert_eq!(completion["response"]["id"], *expected_id);
    }
    eprintln!("loopback slow-handshake request durations (ms): {durations:?}");
    tokio::time::sleep(Duration::from_millis(30)).await;
    let expected_http = expected_ids
        .iter()
        .filter(|id| **id == "safe-http-fallback")
        .count();
    assert_eq!(http_sends.load(Ordering::SeqCst), expected_http);
    assert_eq!(
        ws_sends.load(Ordering::SeqCst),
        expected_ids.len() - expected_http
    );
    assert_eq!(connects.load(Ordering::SeqCst), expected_connects);
    let events = events.lock().unwrap();
    assert_eq!(
        events.len(),
        expected_ids.len(),
        "an unsent optional handshake is not a failed generation"
    );
    assert!(events.iter().all(|event| event.success));
    let transport = serde_json::to_value(&events[0].routing.as_ref().unwrap().transport).unwrap();
    assert_eq!(transport["fallbackReason"], "websocket_slow_handshake");
}

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
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"compact-done\",\"status\":\"completed\",\"output\":[]}}\n\n",
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
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"large-compact\",\"status\":\"completed\",\"output\":[]}}\n\n",
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
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"api fallback\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"http-fallback\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        )])])
        .await;
    let authority = ready_authority("limited-account", "limited-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
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
        GatewayRuntimeOptions {
            diagnostics_enabled: || true,
            ..Default::default()
        },
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
    let transport = serde_json::to_value(&events[1].routing.as_ref().unwrap().transport).unwrap();
    assert_eq!(
        events[1].transport,
        zenith_relay_core::UsageTransport::Websocket
    );
    assert_eq!(transport["upstream"], "http");
    assert_eq!(transport["mode"], "native");
    assert_eq!(transport["responseFormat"], "sse");
    assert!(!transport["fallbackReason"].is_null());
    assert!(transport["handshakeMs"].is_null());
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
async fn disconnect_before_first_websocket_response_releases_lease_without_fallback_or_cooldown() {
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let (upstream, state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::GatedSuccess(barrier.clone()))
            .await;
    let (reserve, reserve_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(2).unwrap());
    register_ready(&authority, "first", "synthetic-first-access").await;
    register_ready(&authority, "reserve", "synthetic-reserve-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first", "synthetic-first", &upstream, 100),
            account("reserve", "synthetic-reserve", &reserve, 0),
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
            json!({
                "type": "response.create", "model": MODEL, "input": "synthetic cancellation"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while state.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("upstream did not receive the request");
    drop(socket);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if gateway
                .runtime
                .as_ref()
                .unwrap()
                .candidate_runtime_order()
                .iter()
                .all(|member| member.in_flight == 0)
                && !events.lock().unwrap().is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("silent upstream kept the disconnected client lease");
    barrier.wait().await;
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert!(reserve_state.requests.lock().unwrap().is_empty());
    let snapshots = gateway.runtime.as_ref().unwrap().candidate_runtime_order();
    assert!(
        snapshots
            .iter()
            .all(|member| member.next_retry_at_ms.is_none())
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].ttft_ms, None);
    assert_eq!(events[0].retry_at_ms, None);
    assert_eq!(
        events[0].routing.as_ref().unwrap().attempt_outcome,
        Some(zenith_relay_core::AttemptOutcome::ClientDisconnected)
    );
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
