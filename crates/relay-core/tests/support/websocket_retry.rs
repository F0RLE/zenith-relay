use super::*;

#[tokio::test]
async fn websocket_upgrade_refreshes_once_on_unauthorized() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let (upstream, state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::UnauthorizedOnce(attempts.clone()),
    )
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "relay-refresh-account",
            TokenSet::new(
                "old-access",
                Some("refresh-secret".into()),
                None,
                Some(current_time_ms() + 600_000),
                current_time_ms(),
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let refresh = Arc::new(RefreshAdapter {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
        access_token: "new-access",
    });
    let (gateway, events, refresh, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "relay-refresh-account",
            "provider-refresh-account",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh,
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
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type":"response.create","model":MODEL,"input":"hello"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(receive_websocket_json(&mut socket).await["delta"], "hello");
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );

    socket
        .send(ClientWsMessage::Text(
            json!({"type":"response.create","model":MODEL,"input":"after refresh"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(receive_websocket_json(&mut socket).await["delta"], "hello");
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );

    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    let headers = state.headers.lock().unwrap();
    assert_eq!(headers.len(), 3);
    assert_eq!(
        header(&headers[0], AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer old-access")
    );
    assert_eq!(
        header(&headers[1], AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer new-access")
    );
    assert_eq!(
        header(&headers[2], AUTHORIZATION.as_str()).as_deref(),
        Some("Bearer new-access")
    );
    drop(headers);
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn websocket_discards_late_response_frames_after_terminal() {
    let (upstream, state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type":"response.output_text.delta","delta":"once"}),
            json!({"type":"response.completed","response":{"id":"ws-once"}}),
            json!({"type":"response.output_text.delta","delta":"duplicate"}),
            json!({"type":"response.completed","response":{"id":"ws-once"}}),
        ])))
        .await;
    let authority = ready_authority("once-account", "once-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("once-account", "provider-once", &upstream, 10)],
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
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type":"response.create","model":MODEL,"input":"one turn"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(receive_websocket_json(&mut socket).await["delta"], "once");
    assert_eq!(
        receive_websocket_json(&mut socket).await["type"],
        "response.completed"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), socket.next())
            .await
            .is_err(),
        "late frames from a completed response reached the client"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn account_websocket_retries_rejection_but_stops_after_unknown_disconnect() {
    let reset_at = current_time_ms() / 1_000 + 6 * 24 * 60 * 60;
    let (status_upstream, status_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type": "response.created", "response": {"id": "discarded-response"}}),
            json!({
                "type": "error",
                "status": 429,
                "body": {"error": {
                    "type": "usage_limit_reached",
                    "message": "Usage limit reached. Try again after the reset.",
                    "resets_at": reset_at
                }}
            }),
        ])))
        .await;
    let (closed_upstream, closed_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Close).await;
    let (success_upstream, success_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "status-account", "status-access").await;
    register_ready(&authority, "closed-account", "closed-access").await;
    register_ready(&authority, "success-account", "success-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("status-account", "provider-status", &status_upstream, 300),
            account("closed-account", "provider-closed", &closed_upstream, 200),
            account(
                "success-account",
                "provider-success",
                &success_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(
        &gateway,
        &["status-account", "closed-account", "success-account"],
    );

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
            json!({"type": "response.create", "model": MODEL, "input": "retry me"}).to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(status_state.requests.lock().unwrap().len(), 1);
    assert_eq!(closed_state.requests.lock().unwrap().len(), 1);
    assert_eq!(success_state.requests.lock().unwrap().len(), 0);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events.iter().map(|event| event.attempt).collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(events[..2]
        .iter()
        .all(|event| event.request_id == events[0].request_id));
    assert_eq!(
        events[0].http_status,
        StatusCode::TOO_MANY_REQUESTS.as_u16()
    );
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_quota_exhausted")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some("*"));
    assert!(events[0].retry_at_ms.is_some());
    assert_eq!(events[0].consecutive_failures, Some(0));
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_websocket_closed")
    );
    assert!(!events[1].success);
}

#[tokio::test]
async fn persistent_websocket_retry_stops_when_the_client_disconnects() {
    let (upstream, upstream_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Close).await;
    let authority = ready_authority("cancel-account", "cancel-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("cancel-account", "provider-cancel", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap();
    runtime.set_route_recovery_enabled(true);

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("originator", "codex_cli_rs")
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "cancel retry"}).to_string(),
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if upstream_state.requests.lock().unwrap().len() == 1 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("persistent retry did not reach the upstream");
    drop(socket);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if runtime.candidate_runtime_order()[0].in_flight == 0 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("client disconnect did not cancel persistent retry");
    assert!(!events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn persistent_websocket_retry_stops_when_retry_policy_is_disabled() {
    let (upstream, upstream_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Close).await;
    let authority = ready_authority("toggle-account", "toggle-access").await;
    let (gateway, _events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("toggle-account", "provider-toggle", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();
    runtime.set_route_recovery_enabled(true);

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("originator", "codex_cli_rs")
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "stop retry"}).to_string(),
        ))
        .await
        .unwrap();

    tokio::time::timeout(Duration::from_secs(2), async {
        while upstream_state.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("persistent retry did not reach the upstream");
    runtime.set_route_recovery_enabled(false);

    let failure = receive_websocket_json_with_timeout(&mut socket, Duration::from_secs(3)).await;
    assert_eq!(failure["type"], "error");
    assert_eq!(failure["error"]["code"], "upstream_websocket_closed");
}

#[tokio::test]
async fn websocket_unknown_terminal_transport_error_is_not_replayed() {
    let (upstream, upstream_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type": "error",
            "status": 502,
            "error": {"code": "upstream_transport", "message": "temporary transport failure"}
        })])))
        .await;
    let authority = ready_authority("terminal-toggle-account", "terminal-toggle-access").await;
    let (gateway, _events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "terminal-toggle-account",
            "provider-terminal-toggle",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("originator", "codex_cli_rs")
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "do not retry"}).to_string(),
        ))
        .await
        .unwrap();

    let failure = receive_websocket_json_with_timeout(&mut socket, Duration::from_secs(2)).await;
    assert_eq!(failure["type"], "error");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(upstream_state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn websocket_http_fallback_releases_its_lease_on_client_disconnect() {
    for after_output in [false, true] {
        let (upstream, requests) = if after_output {
            let (upstream, state) = spawn_held_stream_upstream().await;
            (upstream, state.requests)
        } else {
            let (upstream, state) = spawn_delayed_upstream(Duration::from_secs(30)).await;
            (upstream, state.requests)
        };
        let (gateway, events, _, _) = spawn_mixed_gateway(
            vec![source("cancel-source", &upstream, "source-key", 10)],
            Vec::new(),
            vec![mixed_key(None, None)],
            Arc::new(TokenAuthority::new(4).unwrap()),
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let runtime = gateway.runtime.as_ref().unwrap();
        runtime.set_route_recovery_enabled(true);
        let upgraded = reqwest::Client::new()
            .get(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .header("originator", "codex_cli_rs")
            .upgrade()
            .send()
            .await
            .unwrap();
        let mut socket = upgraded.into_websocket().await.unwrap();
        socket
            .send(ClientWsMessage::Text(
                json!({"type": "response.create", "model": MODEL, "input": "cancel fallback"})
                    .to_string(),
            ))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while requests.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("HTTP fallback did not reach the upstream");
        if after_output {
            assert_eq!(
                receive_websocket_json(&mut socket).await["type"],
                "response.output_text.delta"
            );
        }
        assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 1);
        drop(socket);

        tokio::time::timeout(Duration::from_secs(2), async {
            while runtime.candidate_runtime_order()[0].in_flight != 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("client disconnect left the HTTP fallback lease occupied");
        if after_output {
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 1);
            assert_eq!(
                events[0].error_category.as_deref(),
                Some("client_cancelled")
            );
            assert_eq!(events[0].retry_at_ms, None);
        }
    }
}

#[tokio::test]
async fn account_websocket_does_not_commit_on_a_setup_frame_before_disconnect() {
    let request_count = Arc::new(AtomicUsize::new(0));
    let (upstream, upstream_state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::SuccessThenSetupClose(request_count),
    )
    .await;
    let (reserve_upstream, reserve_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "setup-primary", "setup-primary-access").await;
    register_ready(&authority, "setup-reserve", "setup-reserve-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("setup-primary", "provider-setup-primary", &upstream, 200),
            account(
                "setup-reserve",
                "provider-setup-reserve",
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
    rotation_policy::set_order(&gateway, &["setup-primary", "setup-reserve"]);

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
            json!({"type": "response.create", "model": MODEL, "input": "first request"})
                .to_string(),
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

    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "setup then disconnect"})
                .to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");

    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(upstream_state.requests.lock().unwrap().len(), 2);
    assert_eq!(reserve_state.requests.lock().unwrap().len(), 0);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[0].success);
    assert!(!events[1].success);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_websocket_closed")
    );
    assert_eq!(events[1].account_id.as_deref(), Some("setup-primary"));
    assert_eq!(events[1].retry_at_ms, None);
}
