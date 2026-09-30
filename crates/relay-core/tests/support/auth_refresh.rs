use super::*;

#[tokio::test]
async fn account_headers_use_provider_id_but_usage_keeps_only_local_identity() {
    let (upstream, state) = spawn_upstream(vec![success_reply("account-response")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let runtime_account = account("relay-account", "provider-account-private", &upstream, 10);
    assert!(!format!("{runtime_account:?}").contains("provider-account-private"));
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![runtime_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer account-access")
    );
    assert_eq!(
        requests[0].chatgpt_account_id.as_deref(),
        Some("provider-account-private")
    );
    assert_eq!(requests[0].originator.as_deref(), Some(CODEX_ORIGINATOR));
    assert_eq!(requests[0].body["store"], false);
    assert_eq!(requests[0].body["stream"], true);
    assert!(requests[0].body["input"].is_array());
    assert!(requests[0].body.get("max_output_tokens").is_none());
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events[0].candidate_id.as_deref(), Some("relay-account"));
    assert_eq!(events[0].account_id.as_deref(), Some("relay-account"));
    assert_eq!(events[0].source_id, "openai-codex");
    let serialized = serde_json::to_string(&events[0]).unwrap();
    assert!(!serialized.contains("provider-account-private"));
    assert!(!serialized.contains("account-access"));
}

#[tokio::test]
async fn responses_lite_normalizes_parallel_tools_for_api_sources_too() {
    let (upstream, state) = spawn_upstream(vec![success_reply("source-response")]).await;
    let authority = ready_authority("unused-account", "unused-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source("api-source", &upstream, "source-key", 10)],
        Vec::new(),
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-openai-internal-codex-responses-lite", "true")
        .json(&json!({
            "model": MODEL,
            "input": "hello",
            "parallel_tool_calls": true
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].body["parallel_tool_calls"], false);
    assert!(requests[0].responses_lite.is_none());
}

#[tokio::test]
async fn chatgpt_account_retries_foreign_message_item_id_after_native_rejection() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::BAD_REQUEST,
            json!({"error": {
                "message": "Invalid 'input[151].id': 'item_foreign_user_01'. Expected an ID that begins with 'msg'."
            }}),
        ),
        success_reply("message-id-repaired"),
    ])
    .await;
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

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [
                {
                    "type": "message",
                    "id": "item_foreign_user_01",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "Inspect the workspace"}]
                },
                {
                    "id": "item_foreign_assistant_01",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "I will inspect it."}]
                },
                {
                    "type": "message",
                    "id": "msg_native_01",
                    "role": "developer",
                    "content": [{"type": "input_text", "text": "Keep changes scoped."}]
                },
                {
                    "type": "function_call",
                    "id": "item_function_01",
                    "call_id": "call_function_01",
                    "name": "run_command",
                    "arguments": "{\"command\":\"pwd\"}"
                }
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], "message-id-repaired");

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].body["input"][0]["id"], "item_foreign_user_01");
    assert_eq!(
        requests[0].body["input"][1]["id"],
        "item_foreign_assistant_01"
    );
    assert!(requests[1].body["input"][0].get("id").is_none());
    assert!(requests[1].body["input"][1].get("id").is_none());
    assert_eq!(requests[1].body["input"][2]["id"], "msg_native_01");
    assert_eq!(requests[1].body["input"][3]["id"], "item_function_01");
    assert_eq!(requests[1].body["input"][3]["call_id"], "call_function_01");
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
}

#[tokio::test]
async fn unauthorized_account_request_refreshes_once_and_retries_the_same_account() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"token_expired"}}),
        ),
        success_reply("refreshed-response"),
    ])
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
        authority.clone(),
        refresh,
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    {
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].authorization.as_deref(),
            Some("Bearer old-access")
        );
        assert_eq!(
            requests[1].authorization.as_deref(),
            Some("Bearer new-access")
        );
        assert!(requests.iter().all(|request| {
            request.chatgpt_account_id.as_deref() == Some("provider-refresh-account")
        }));
    }
    {
        let recorded = events.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        assert!(recorded[0].success);
    }

    assert_eq!(
        authority.auth_state("relay-refresh-account").await,
        Some(AccountAuthState::Active)
    );
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(
        requests[2].authorization.as_deref(),
        Some("Bearer new-access")
    );
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events[1].success);
}

#[tokio::test]
async fn unauthorized_sse_refreshes_before_output_and_emits_one_completion() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"token_expired"}}),
        ),
        Reply::Stream(vec![
            StreamChunk::Data("data: {\"type\":\"response.created\",\"response\":{\"id\":\"refreshed\"}}\n\n"),
            StreamChunk::Data("data: {\"type\":\"response.output_text.delta\",\"delta\":\"once\"}\n\n"),
            StreamChunk::Data("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"refreshed\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n"),
        ]),
    ])
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

    let response = request(&gateway, true).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = response.text().await.unwrap();
    let frames = body
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str::<Value>(data).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(frames.len(), 3);
    assert_eq!(frames[1]["delta"], "once");
    assert_eq!(frames[2]["type"], "response.completed");
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer old-access")
    );
    assert_eq!(
        requests[1].authorization.as_deref(),
        Some("Bearer new-access")
    );
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].total_tokens, Some(2));
}

#[tokio::test]
async fn auth_replay_uses_the_same_three_dispatch_budget_as_route_fallback() {
    let (first_upstream, first_state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"token_expired"}}),
        ),
        Reply::Json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error":{"code":"server_is_overloaded"}}),
        ),
    ])
    .await;
    let (second_upstream, second_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error":{"code":"server_is_overloaded"}}),
    )])
    .await;
    let (third_upstream, third_state) =
        spawn_upstream(vec![success_reply("budget-must-not-reach-this-account")]).await;
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
    register_ready(&authority, "second", "second-access").await;
    register_ready(&authority, "third", "third-access").await;
    let (gateway, _, refresh, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![
            account(
                "relay-refresh-account",
                "provider-first",
                &first_upstream,
                300,
            ),
            account("second", "provider-second", &second_upstream, 200),
            account("third", "provider-third", &third_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        Arc::new(RefreshAdapter {
            calls: AtomicUsize::new(0),
            delay: Duration::ZERO,
            access_token: "new-access",
        }),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions::default(),
    )
    .await;
    rotation_policy::set_order(&gateway, &["relay-refresh-account", "second", "third"]);

    assert_ne!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    assert_eq!(first_state.requests.lock().unwrap().len(), 2);
    assert_eq!(second_state.requests.lock().unwrap().len(), 1);
    assert!(third_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn passive_quota_headers_update_the_active_runtime_before_persistence() {
    let (upstream, _) = spawn_upstream(vec![Reply::JsonWithHeaders(
        StatusCode::OK,
        json!({
            "id":"quota-response",
            "object":"response",
            "model":MODEL,
            "output":[],
            "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
        }),
        vec![
            ("x-codex-primary-used-percent", "100"),
            ("x-codex-primary-reset-after-seconds", "60"),
        ],
    )])
    .await;
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

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(
        request(&gateway, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    let quota = events[0].quota_snapshot.as_ref().unwrap();
    assert!(quota.limit_reached);
    assert_eq!(
        quota.primary.as_ref().unwrap().available_basis_points,
        Some(0)
    );
}
