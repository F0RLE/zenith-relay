use super::*;

#[tokio::test]
async fn api_route_recovery_waits_for_configured_models_on_all_text_protocols() {
    let cases = [
        ("/v1/responses", json!({"model": MODEL, "input": "hello"})),
        (
            "/v1/chat/completions",
            json!({"model": MODEL, "messages": [{"role": "user", "content": "hello"}]}),
        ),
        (
            "/v1/messages",
            json!({"model": MODEL, "max_tokens": 32, "messages": [{"role": "user", "content": "hello"}]}),
        ),
        (
            "/v1beta/models/gpt-p3:generateContent",
            json!({"contents": [{"role": "user", "parts": [{"text": "hello"}]}]}),
        ),
    ];
    for (path, body) in cases {
        let (upstream, upstream_state) = spawn_upstream(vec![Reply::Json(
            StatusCode::OK,
            json!({
                "id": "recovered", "object": "response", "status": "completed", "model": MODEL,
                "output": [{"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ready"}]}],
                "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
            }),
        )]).await;
        let mut api_source = source("recovery-source", &upstream, "source-key", 10);
        api_source.protocol_bindings = [
            (WireApi::Responses, SourceAdapter::Native),
            (
                WireApi::ChatCompletions,
                SourceAdapter::ChatCompletionsToResponses,
            ),
            (WireApi::Messages, SourceAdapter::MessagesToResponses),
            (WireApi::Gemini, SourceAdapter::GeminiToResponses),
        ]
        .into_iter()
        .map(|(wire_api, adapter)| SourceProtocolBinding {
            wire_api,
            adapter,
            reasoning_mode: Default::default(),
            cache_write_ttl: Default::default(),
            model_ids: vec![MODEL.into()],
        })
        .collect();
        let (gateway, _, _, _) = spawn_mixed_gateway(
            vec![api_source],
            Vec::new(),
            vec![mixed_key(None, None)],
            Arc::new(TokenAuthority::new(4).unwrap()),
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let runtime = gateway.runtime.as_ref().unwrap().clone();
        let candidates = runtime
            .candidate_runtime_order()
            .into_iter()
            .map(|candidate| candidate.candidate_id)
            .collect::<Vec<_>>();
        assert!(!candidates.is_empty());
        for candidate in &candidates {
            assert!(runtime.set_candidate_health(candidate, CandidateHealth::Unhealthy));
        }
        let client = reqwest::Client::new();
        let url = format!("{}{path}", gateway.base_url);
        let send = |client: reqwest::Client, url: String, body: Value| async move {
            client
                .post(url)
                .bearer_auth(LOCAL_KEY)
                .json(&body)
                .send()
                .await
                .unwrap()
        };
        let unavailable = send(client.clone(), url.clone(), body.clone()).await;
        assert_eq!(unavailable.status(), StatusCode::NOT_FOUND, "{path}");

        runtime.set_route_recovery_enabled(true);
        let pending = tokio::spawn(send(client, url, body));
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(
            !pending.is_finished(),
            "{path} should wait without a ChatGPT header"
        );
        assert!(upstream_state.requests.lock().unwrap().is_empty());
        for candidate in &candidates {
            assert!(runtime.set_candidate_health(candidate, CandidateHealth::Healthy));
        }
        let response = tokio::time::timeout(Duration::from_secs(3), pending)
            .await
            .expect("route recovery did not wake")
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{path}: {}",
            response.text().await.unwrap()
        );
        assert_eq!(upstream_state.requests.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn non_chatgpt_websocket_waits_for_route_recovery() {
    let (upstream, upstream_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Success).await;
    let authority = ready_authority("recovery-account", "recovery-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "recovery-account",
            "provider-recovery",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap();
    runtime.set_route_recovery_enabled(true);
    assert!(runtime.set_candidate_health("recovery-account", CandidateHealth::Unhealthy));

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
            json!({"type": "response.create", "model": MODEL, "input": "wait"}).to_string(),
        ))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(upstream_state.requests.lock().unwrap().is_empty());
    assert!(runtime.set_candidate_health("recovery-account", CandidateHealth::Healthy));
    let first = tokio::time::timeout(Duration::from_secs(3), receive_websocket_json(&mut socket))
        .await
        .expect("websocket route recovery did not wake");
    assert_eq!(first["type"], "response.output_text.delta");
    assert_eq!(
        receive_websocket_completion(&mut socket).await["type"],
        "response.completed"
    );
    assert_eq!(upstream_state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn retry_cap_stops_account_rotation_after_429() {
    let (limited_upstream, limited_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"code": "rate_limit_exceeded"}}),
    )])
    .await;
    let (ready_upstream, ready_state) =
        spawn_upstream(vec![success_reply("rotated-response")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "limited-account", "limited-access").await;
    register_ready(&authority, "ready-account", "ready-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account(
                "limited-account",
                "provider-limited",
                &limited_upstream,
                200,
            ),
            account("ready-account", "provider-ready", &ready_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "rate_limit_exceeded"
    );
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    assert!(ready_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn retry_cap_stops_compact_account_rotation_after_429() {
    let (limited_upstream, limited_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"code": "rate_limit_exceeded"}}),
    )])
    .await;
    let (ready_upstream, ready_state) =
        spawn_upstream(vec![success_reply("rotated-response")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "limited-account", "limited-access").await;
    register_ready(&authority, "ready-account", "ready-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account(
                "limited-account",
                "provider-limited",
                &limited_upstream,
                200,
            ),
            account("ready-account", "provider-ready", &ready_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "compact this",
            "max_output_tokens": 16,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "rate_limit_exceeded"
    );
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    assert!(ready_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn local_key_account_scope_uses_relay_ids_not_provider_header_ids() {
    let (allowed_upstream, allowed_state) = spawn_upstream(vec![success_reply("allowed")]).await;
    let (denied_upstream, denied_state) = spawn_upstream(vec![success_reply("denied")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "allowed-local", "allowed-access").await;
    register_ready(&authority, "denied-local", "denied-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("denied-local", "provider-denied", &denied_upstream, 100),
            account("allowed-local", "provider-allowed", &allowed_upstream, 0),
        ],
        vec![mixed_key(None, Some(vec!["allowed-local"]))],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(models(&gateway).await, [MODEL]);
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(allowed_state.requests.lock().unwrap().len(), 1);
    assert!(denied_state.requests.lock().unwrap().is_empty());
    assert_eq!(
        allowed_state.requests.lock().unwrap()[0]
            .chatgpt_account_id
            .as_deref(),
        Some("provider-allowed")
    );
}

#[tokio::test]
async fn concurrent_gateway_requests_rotate_and_persist_one_token_once() {
    let (upstream, state) = spawn_upstream(Vec::new()).await;
    let authority = Arc::new(TokenAuthority::new(2).unwrap());
    authority
        .register(
            "relay-refresh-account",
            TokenSet::new(
                "expired-access",
                Some("refresh-secret".into()),
                None,
                Some(1),
                0,
                0,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let refresh = Arc::new(RefreshAdapter {
        calls: AtomicUsize::new(0),
        delay: Duration::from_millis(25),
        access_token: "rotated-access",
    });
    let persistence = Arc::new(PersistenceAdapter::default());
    let (gateway, _, refresh, persistence) = spawn_mixed_gateway(
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
        persistence,
    )
    .await;

    let client = reqwest::Client::new();
    let responses = join_all((0..20).map(|_| {
        let client = client.clone();
        let url = format!("{}/v1/responses", gateway.base_url);
        async move {
            client
                .post(url)
                .bearer_auth(LOCAL_KEY)
                .json(&json!({"model": MODEL, "input": "hello"}))
                .send()
                .await
                .unwrap()
        }
    }))
    .await;
    assert!(responses
        .iter()
        .all(|response| response.status() == StatusCode::OK));
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    assert_eq!(persistence.token_writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        *persistence.persisted_accounts.lock().unwrap(),
        vec!["relay-refresh-account".to_string()]
    );
    assert_eq!(
        *persistence.auth_states.lock().unwrap(),
        vec![(
            "relay-refresh-account".to_string(),
            AccountAuthState::Active
        )]
    );
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 20);
    assert!(requests
        .iter()
        .all(|request| request.authorization.as_deref() == Some("Bearer rotated-access")));
}

#[tokio::test]
async fn concurrent_new_chats_are_balanced_across_equal_accounts() {
    const REQUESTS: usize = 200;
    let (first_upstream, first_state) = spawn_delayed_upstream(Duration::from_millis(100)).await;
    let (second_upstream, second_state) = spawn_delayed_upstream(Duration::from_millis(100)).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "account-first", "first-access").await;
    register_ready(&authority, "account-second", "second-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("account-first", "provider-first", &first_upstream, 10),
            account("account-second", "provider-second", &second_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let responses = join_all((0..REQUESTS).map(|_| request(&gateway, false))).await;

    assert!(responses
        .iter()
        .all(|response| response.status() == StatusCode::OK));
    assert_eq!(first_state.requests.lock().unwrap().len(), REQUESTS / 2);
    assert_eq!(second_state.requests.lock().unwrap().len(), REQUESTS / 2);
    assert!(gateway
        .runtime
        .as_ref()
        .unwrap()
        .candidate_runtime_order()
        .iter()
        .all(|candidate| candidate.in_flight == 0));
}

#[tokio::test]
async fn independent_chat_stays_on_the_account_with_known_quota() {
    let (stream_upstream, stream_state) = spawn_held_then_json_upstream().await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("source-response")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "stream-account", "stream-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source(
            "z-reserve-api",
            &source_upstream,
            "source-key",
            -1_000_000,
        )],
        vec![account(
            "stream-account",
            "provider-stream",
            &stream_upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let open_stream = request(&gateway, true).await;
    assert_eq!(open_stream.status(), StatusCode::OK);
    assert_eq!(stream_state.requests.lock().unwrap().len(), 1);

    let independent = tokio::time::timeout(Duration::from_secs(2), request(&gateway, false))
        .await
        .expect("the independent chat did not stay on the account with known quota");
    assert_eq!(independent.status(), StatusCode::OK);
    let _ = independent.bytes().await.unwrap();
    assert_eq!(stream_state.requests.lock().unwrap().len(), 2);
    assert_eq!(source_state.requests.lock().unwrap().len(), 0);

    stream_state.release.notify_one();
    let _ = open_stream.bytes().await.unwrap();
    tokio::time::sleep(Duration::from_millis(20)).await;
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| {
        event.account_id.as_deref() == Some("stream-account") && event.source_id != "z-reserve-api"
    }));
}

#[tokio::test]
async fn queued_request_rechecks_scope_after_pool_member_removal() {
    let (removed_upstream, removed_state) = spawn_held_stream_upstream().await;
    let (blocker_upstream, blocker_state) = spawn_held_then_json_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "removed-account", "removed-access").await;
    register_ready(&authority, "blocker-account", "blocker-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("removed-account", "provider-removed", &removed_upstream, 10),
            account("blocker-account", "provider-blocker", &blocker_upstream, 10),
        ],
        vec![mixed_key(None, Some(vec!["removed-account"]))],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();

    let removed_request = request(&gateway, true).await;
    assert_eq!(removed_request.status(), StatusCode::OK);
    assert_eq!(removed_state.requests.lock().unwrap().len(), 1);

    assert!(runtime.update_key_scope(
        "local-key",
        CandidateScope {
            source_ids: Some(BTreeSet::new()),
            account_ids: Some(
                ["removed-account", "blocker-account"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
            ),
            model_rules: Default::default(),
        },
    ));
    let blocker_request = request(&gateway, true).await;
    assert_eq!(blocker_request.status(), StatusCode::OK);
    assert_eq!(blocker_state.requests.lock().unwrap().len(), 1);

    assert!(runtime.update_key_scope(
        "local-key",
        CandidateScope {
            source_ids: Some(BTreeSet::new()),
            account_ids: Some(BTreeSet::from(["blocker-account".to_string()])),
            model_rules: Default::default(),
        },
    ));

    let queued_response = tokio::time::timeout(Duration::from_secs(2), request(&gateway, false))
        .await
        .expect("the scoped request did not stay on the allowed account");
    assert_eq!(queued_response.status(), StatusCode::OK);
    let _ = queued_response.bytes().await.unwrap();
    assert_eq!(removed_state.requests.lock().unwrap().len(), 1);
    assert_eq!(blocker_state.requests.lock().unwrap().len(), 2);

    blocker_state.release.notify_waiters();
    let _ = blocker_request.bytes().await.unwrap();
    removed_state.release.notify_waiters();
    let _ = removed_request.bytes().await.unwrap();
    assert_eq!(removed_state.requests.lock().unwrap().len(), 1);
    assert_eq!(blocker_state.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn oauth_accounts_use_independent_upstream_connection_pools() {
    let (upstream, state) = spawn_connection_affinity_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "first-account", "first-access").await;
    register_ready(&authority, "second-account", "second-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account("first-account", "provider-first", &upstream, 100),
            account("second-account", "provider-second", &upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            max_retry_candidates: 1,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);

    let account_ids = state.account_ids.lock().unwrap();
    assert_eq!(account_ids.len(), 2);
    assert_ne!(account_ids[0], account_ids[1]);
    assert_eq!(state.owners.lock().unwrap().len(), 2);
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn cancelled_sse_releases_its_lease_without_cooling_the_account() {
    let (upstream, state) = spawn_held_stream_upstream().await;
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

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        gateway.runtime.as_ref().unwrap().candidate_runtime_order()[0].in_flight,
        1
    );
    drop(response);

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
    .expect("cancelled SSE lease was not released");
    state.release.notify_waiters();
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("client_cancelled")
    );
    assert_eq!(events[0].retry_at_ms, None);
}

#[tokio::test]
async fn pre_output_failure_releases_only_replayable_tool_affinity() {
    for stream in [false, true] {
        for (include_call, previous_response) in [(true, false), (false, false), (true, true)] {
            for (failure, replay_safe) in [
                (Reply::Json(
                    StatusCode::SERVICE_UNAVAILABLE,
                    json!({"error": {"code": "server_is_overloaded"}}),
                ), true),
                (Reply::Json(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error": {"type": "server_error", "code": "server_error"}}),
                ), false),
                (Reply::Stream(vec![
                    StreamChunk::Data("data: {\"type\":\"response.created\",\"response\":{\"id\":\"failed-setup\"}}\n\n"),
                    StreamChunk::Data("data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"type\":\"server_error\",\"code\":\"server_error\"}}}\n\n"),
                ]), false),
                (Reply::Stream(vec![StreamChunk::Error]), false),
            ] {
                assert_tool_affinity_retry(stream, include_call, previous_response, failure, replay_safe)
                    .await;
            }
        }
    }
}

async fn assert_tool_affinity_retry(
    stream: bool,
    include_call: bool,
    previous_response: bool,
    failure: Reply,
    replay_safe: bool,
) {
    // Complete tool history proves portability, not execution safety. Generic
    // 5xx, terminal errors and broken streams must not create a second generation.
    let fallback_expected = include_call && !previous_response && replay_safe;
    let call = json!({
        "type": "function_call", "id": "fc_test", "call_id": "call_test",
        "name": "lookup", "arguments": "{}"
    });
    let (owner, owner_state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::OK,
            json!({
                "id": "tool-owner-response", "object": "response", "model": MODEL,
                "output": [call.clone()]
            }),
        ),
        failure.clone(),
        failure.clone(),
        failure,
    ])
    .await;
    let (backup, backup_state) = spawn_upstream(vec![if stream {
        successful_sse_reply()
    } else {
        success_reply("backup-response")
    }])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner", "owner-access").await;
    register_ready(&authority, "backup", "backup-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner", "provider-owner", &owner, 10_000),
            account("backup", "provider-backup", &backup, 1_000),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(&gateway, &["owner", "backup"]);
    let first = request(&gateway, false).await;
    assert_eq!(first.status(), StatusCode::OK);
    let _: Value = first.json().await.unwrap();
    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);

    let mut input = Vec::new();
    if include_call {
        input.push(call);
    }
    input.push(json!({"type": "function_call_output", "call_id": "call_test", "output": "result"}));
    let mut body = json!({"model": MODEL, "stream": stream, "input": input});
    if previous_response {
        body["previous_response_id"] = json!("tool-owner-response");
    }
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    let fallback = status.is_success();
    if !previous_response || !replay_safe {
        assert_eq!(
            fallback,
            fallback_expected,
            "stream={stream}, include_call={include_call}, previous_response={previous_response}, status={status}, body={}",
            String::from_utf8_lossy(&bytes)
        );
    }
    assert!(!String::from_utf8_lossy(&bytes).contains("failed-setup"));
    let owner_attempts = owner_state.requests.lock().unwrap().len();
    if replay_safe && !include_call && !previous_response {
        // An unpaired result stays with its owner. The owner may recover, but
        // this must stay inside the same three-send request budget.
        assert!((2..=4).contains(&owner_attempts));
    } else {
        assert_eq!(owner_attempts, 2);
    }
    assert_eq!(
        backup_state.requests.lock().unwrap().len(),
        usize::from(fallback)
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), owner_attempts + usize::from(fallback));
    assert_eq!(
        events[1].routing.as_ref().unwrap().reason,
        SelectionReason::ResponseAffinity
    );
    assert!(!events[1].success);
    assert_eq!(events[1].ttft_ms, None);
    if fallback {
        assert!(events[2].success);
        assert_eq!(events[2].attempt, 2);
        assert_eq!(events[1].request_id, events[2].request_id);
        assert_ne!(events[1].candidate_id, events[2].candidate_id);
    }
}

#[tokio::test]
async fn account_stream_never_falls_back_after_first_event() {
    let (account_upstream, account_state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data("data: {\"type\":\"response.output_text.delta\",\"delta\":\"ok\"}\n\n"),
        StreamChunk::Error,
    ])])
    .await;
    let (source_upstream, source_state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("source", &source_upstream, "source-key", 0)],
        vec![account(
            "relay-account",
            "provider-account",
            &account_upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "text/event-stream"
    );
    let _ = response.bytes().await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(account_state.requests.lock().unwrap().len(), 1);
    assert!(source_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].account_id.as_deref(), Some("relay-account"));
    assert!(!events[0].success);
}

#[tokio::test]
async fn agent_identity_account_signs_each_gateway_request() {
    let (upstream, state) = spawn_upstream(vec![success_reply("ok")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    let mut agent_identities = HashMap::new();
    agent_identities.insert(
        "relay-agent".to_string(),
        AgentIdentityCredential::new(
            "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g".into(),
            "runtime-test".into(),
            "task-test".into(),
        )
        .unwrap(),
    );
    let (gateway, _, _, _) = spawn_mixed_gateway_with_agent_identities(
        Vec::new(),
        vec![account("relay-agent", "provider-agent", &upstream, 10_000)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        agent_identities,
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0]
        .authorization
        .as_deref()
        .is_some_and(|value| value.starts_with("AgentAssertion ")));
}
