use super::*;

#[tokio::test]
async fn mixed_responses_routes_disable_automatic_lite_but_preserve_explicit_client_lite() {
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("api-source-must-not-run")]).await;
    let mut account_catalog = default_upstream_model_catalog();
    account_catalog["models"][0]["slug"] = Value::String(OFFICIAL_CODEX_MODEL.to_string());
    let (account_upstream, account_state) = spawn_upstream_with_catalog(
        vec![
            success_reply("full-response"),
            success_reply("explicit-lite-response"),
        ],
        account_catalog,
    )
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let mut api_source = source("api-source", &source_upstream, "source-key", -1_000_000);
    api_source.source.models = vec![OFFICIAL_CODEX_MODEL.to_string()];
    let mut oauth_account = account("relay-account", "provider-account", &account_upstream, 10);
    oauth_account.models = vec![OFFICIAL_CODEX_MODEL.to_string()];
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![api_source],
        vec![oauth_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(&gateway, &["relay-account", "api-source"]);
    let client = reqwest::Client::new();

    // Populate the account-owned catalog metadata that identifies this Codex
    // model as a Lite-capable one. The same scoped key can still route it to
    // the API source above, so automatic Lite must stay off.
    assert_eq!(
        client
            .get(format!(
                "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
                gateway.base_url
            ))
            .bearer_auth(LOCAL_KEY)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let request_body = json!({
        "model": OFFICIAL_CODEX_MODEL,
        "input": "keep the full Responses contract",
        "parallel_tool_calls": true,
        "reasoning": {"effort": "high"}
    });
    assert_eq!(
        client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&request_body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .header("x-openai-internal-codex-responses-lite", "true")
            .json(&request_body)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    assert!(source_state
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.path == "/v1/models"));
    let requests = account_state.requests.lock().unwrap();
    let response_requests = requests
        .iter()
        .filter(|request| request.path == "/v1/responses")
        .collect::<Vec<_>>();
    assert_eq!(response_requests.len(), 2);
    assert_eq!(response_requests[0].responses_lite, None);
    assert_eq!(response_requests[0].body["parallel_tool_calls"], true);
    assert!(response_requests[0].body["reasoning"]
        .get("context")
        .is_none());
    assert_eq!(response_requests[1].responses_lite.as_deref(), Some("true"));
    assert_eq!(response_requests[1].body["parallel_tool_calls"], false);
    assert_eq!(
        response_requests[1].body["reasoning"]["context"],
        "all_turns"
    );
}

#[tokio::test]
async fn account_only_compaction_requires_unanimous_automatic_lite_support() {
    let mut lite_catalog = default_upstream_model_catalog();
    lite_catalog["models"][0]["slug"] = Value::String(OFFICIAL_CODEX_MODEL.to_string());
    let (lite_upstream, lite_state) =
        spawn_upstream_with_catalog(vec![success_reply("full-compact")], lite_catalog).await;
    let mut full_catalog = default_upstream_model_catalog();
    full_catalog["models"][0]["slug"] = Value::String(OFFICIAL_CODEX_MODEL.to_string());
    full_catalog["models"][0]["use_responses_lite"] = Value::Bool(false);
    let (full_upstream, full_state) = spawn_upstream_with_catalog(Vec::new(), full_catalog).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "lite-account", "lite-access").await;
    register_ready(&authority, "full-account", "full-access").await;
    let mut lite_account = account("lite-account", "provider-lite", &lite_upstream, 100);
    lite_account.models = vec![OFFICIAL_CODEX_MODEL.to_string()];
    let mut full_account = account("full-account", "provider-full", &full_upstream, 10);
    full_account.models = vec![OFFICIAL_CODEX_MODEL.to_string()];
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![lite_account, full_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(&gateway, &["lite-account", "full-account"]);
    let client = reqwest::Client::new();

    // Both account manifests are needed: the selected account advertises Lite,
    // but an eligible fallback does not. The compact path must therefore retain
    // the full Responses contract just like regular HTTP and WebSocket calls.
    assert_eq!(
        client
            .get(format!(
                "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
                gateway.base_url
            ))
            .bearer_auth(LOCAL_KEY)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        client
            .post(format!("{}/v1/responses/compact", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({
                "model": OFFICIAL_CODEX_MODEL,
                "input": "keep the full compact contract",
                "parallel_tool_calls": true,
                "reasoning": {"effort": "high"}
            }))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let requests = lite_state.requests.lock().unwrap();
    let compact_requests = requests
        .iter()
        .filter(|request| request.path == "/v1/responses/compact")
        .collect::<Vec<_>>();
    assert_eq!(compact_requests.len(), 1);
    assert_eq!(compact_requests[0].responses_lite, None);
    assert_eq!(compact_requests[0].body["parallel_tool_calls"], true);
    assert!(compact_requests[0].body["reasoning"]
        .get("context")
        .is_none());
    drop(requests);
    let full_requests = full_state.requests.lock().unwrap();
    assert_eq!(full_requests.len(), 1);
    assert_eq!(full_requests[0].path, "/v1/models");
}

#[tokio::test]
async fn compact_account_requests_keep_compact_transport_separate_from_responses_defaults() {
    let (upstream, state) = spawn_upstream(vec![Reply::RejectCompactTransportFields]).await;
    let authority = ready_authority("compact-account", "compact-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "compact-account",
            "provider-compact",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "compact this",
            "store": false,
            "stream": false,
            "max_output_tokens": 4,
            "future_compaction_option": {"enabled": true}
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let body = &requests[0].body;
    assert!(body.get("store").is_none());
    assert!(body.get("stream").is_none());
    assert!(body.get("max_output_tokens").is_none());
    assert_eq!(body["future_compaction_option"]["enabled"], true);
    assert_eq!(body["input"][0]["content"][0]["text"], "compact this");
}

#[tokio::test]
async fn mixed_websocket_routes_disable_automatic_lite_but_preserve_explicit_client_lite() {
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("api-source-must-not-run")]).await;
    let mut account_catalog = default_upstream_model_catalog();
    account_catalog["models"][0]["slug"] = Value::String(OFFICIAL_CODEX_MODEL.to_string());
    let (account_upstream, account_state) =
        spawn_websocket_upstream_with_catalog(WebSocketBehavior::Success, account_catalog).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let mut api_source = source("api-source", &source_upstream, "source-key", -1_000_000);
    api_source.source.models = vec![OFFICIAL_CODEX_MODEL.to_string()];
    let mut oauth_account = account("relay-account", "provider-account", &account_upstream, 10);
    oauth_account.models = vec![OFFICIAL_CODEX_MODEL.to_string()];
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![api_source],
        vec![oauth_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let client = reqwest::Client::new();

    // Fetching the native manifest marks the OAuth route as Lite-capable. The
    // same key can nevertheless fall back to the API source, so the automatic
    // path must use full Responses on both transports.
    assert_eq!(
        client
            .get(format!(
                "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
                gateway.base_url
            ))
            .bearer_auth(LOCAL_KEY)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );

    let request = json!({
        "type": "response.create",
        "model": OFFICIAL_CODEX_MODEL,
        "input": "keep the full Responses contract",
        "parallel_tool_calls": true,
        "reasoning": {"effort": "high"}
    });
    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(request.to_string()))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_completion(&mut socket).await["type"],
        "response.completed"
    );
    drop(socket);

    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-openai-internal-codex-responses-lite", "true")
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(request.to_string()))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_completion(&mut socket).await["type"],
        "response.completed"
    );

    assert!(source_state
        .requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| request.path == "/v1/models"));
    let headers = account_state.headers.lock().unwrap();
    assert_eq!(headers.len(), 2);
    assert!(headers[0]
        .get("x-openai-internal-codex-responses-lite")
        .is_none());
    assert_eq!(
        header(&headers[1], "x-openai-internal-codex-responses-lite").as_deref(),
        Some("true")
    );
    drop(headers);

    let requests = account_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["parallel_tool_calls"], true);
    assert!(requests[0]["reasoning"].get("context").is_none());
    assert_eq!(requests[1]["parallel_tool_calls"], false);
    assert_eq!(requests[1]["reasoning"]["context"], "all_turns");
}

#[tokio::test]
async fn account_requests_preserve_responses_lite_compatibility() {
    let (upstream, state) = spawn_upstream(vec![success_reply("lite-response")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            default_service_tier: DefaultServiceTier::Fast,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-openai-internal-codex-responses-lite", "true")
        .json(&json!({
            "model": MODEL,
            "input": "hello",
            "service_tier": "flex",
            "parallel_tool_calls": true,
            "reasoning": {"effort": "high"},
            "tools": [
                {"type": "function", "name": "local_tool"},
                {"type": "web_search"},
                {"type": "image_generation"}
            ]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests[0].responses_lite.as_deref(), Some("true"));
    assert_eq!(requests[0].body["service_tier"], "flex");
    assert_eq!(requests[0].body["parallel_tool_calls"], false);
    let tools = requests[0].body["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3);
    assert_eq!(tools[0]["name"], "local_tool");
    assert_eq!(tools[1]["type"], "web_search");
    assert_eq!(tools[2]["type"], "image_generation");
    assert_eq!(requests[0].body["reasoning"]["context"], "all_turns");
    assert_eq!(requests[0].body["reasoning"]["effort"], "high");
}

#[tokio::test]
async fn compact_and_alpha_search_use_the_oauth_account_runtime() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::OK,
            json!({
                "type": "compaction",
                "items": [],
                "usage": {
                    "input_tokens": 1200,
                    "input_tokens_details": {"cached_tokens": 800},
                    "output_tokens": 100,
                    "total_tokens": 1300
                }
            }),
        ),
        Reply::Json(StatusCode::OK, json!({"results": [{"title": "result"}]})),
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
    let client = reqwest::Client::new();

    let compact = client
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("x-openai-internal-codex-responses-lite", "true")
        .json(&json!({
            "model": MODEL,
            "input": "compact this",
            "stream": false,
            "max_output_tokens": 4
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(compact.status(), StatusCode::OK);
    assert_eq!(compact.json::<Value>().await.unwrap()["type"], "compaction");

    let search = client
        .post(format!("{}/v1/alpha/search", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("user-agent", "synthetic-codex")
        .json(&json!({
            "model": MODEL,
            "id": "session-42",
            "query": "search query",
            "prompt_cache_key": "local-only",
            "prompt_cache_retention": "24h"
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::OK);
    assert_eq!(
        search.json::<Value>().await.unwrap()["results"][0]["title"],
        "result"
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/v1/responses/compact");
    assert_eq!(
        requests[0].authorization.as_deref(),
        Some("Bearer account-access")
    );
    assert_eq!(requests[0].responses_lite.as_deref(), Some("true"));
    assert!(requests[0].body.get("stream").is_none());
    assert!(requests[0].body.get("max_output_tokens").is_none());
    assert_eq!(requests[1].path, "/v1/alpha/search");
    assert_eq!(requests[1].session_id.as_deref(), Some("session-42"));
    assert!(requests[1].body.get("prompt_cache_key").is_none());
    assert!(requests[1].body.get("prompt_cache_retention").is_none());
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
    assert_eq!(events[0].input_tokens, Some(1200));
    assert_eq!(events[0].cached_input_tokens, Some(800));
    assert_eq!(events[0].output_tokens, Some(100));
    assert_eq!(events[0].total_tokens, Some(1300));
    assert!(events
        .iter()
        .all(|event| event.account_id.as_deref() == Some("relay-account")));
}

#[tokio::test]
async fn account_wake_is_pinned_to_its_account_and_reuses_runtime_execution() {
    let (upstream, state) = spawn_upstream(vec![success_reply("wake-response")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "relay-refresh-account",
            TokenSet::new(
                "old-access",
                Some("refresh-secret".into()),
                None,
                Some(current_time_ms().saturating_sub(1)),
                current_time_ms().saturating_sub(2),
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    register_ready(&authority, "other-account", "other-access").await;
    let refresh = Arc::new(RefreshAdapter {
        calls: AtomicUsize::new(0),
        delay: Duration::ZERO,
        access_token: "new-access",
    });
    let (gateway, events, refresh, _) = spawn_mixed_gateway(
        // A permitted API source and a second OAuth account make this a
        // regression for accidental pool fallback. The wake may use only its
        // requested OAuth account.
        vec![source("fallback-source", &upstream, "source-key", -10)],
        vec![
            account(
                "relay-refresh-account",
                "provider-wake-account",
                &upstream,
                10,
            ),
            account("other-account", "provider-other-account", &upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh,
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();

    let response = gateway::execute_account_wake(
        runtime,
        gateway::AccountWakeRequest {
            local_key_id: "system-gateway-key".into(),
            account_id: "relay-refresh-account".into(),
            model_id: MODEL.into(),
            output_token_cap: 8,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["id"],
        "wake-response"
    );

    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(
        requests[0].chatgpt_account_id.as_deref(),
        Some("provider-wake-account")
    );
    assert_eq!(requests[0].originator.as_deref(), Some(CODEX_ORIGINATOR));
    assert!(requests[0].responses_lite.is_none());
    assert_eq!(requests[0].body["model"], MODEL);
    assert_eq!(requests[0].body["stream"], false);
    assert_eq!(requests[0].body["store"], false);
    assert_eq!(requests[0].body["max_output_tokens"], 8);
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(
        events[0].account_id.as_deref(),
        Some("relay-refresh-account")
    );
    assert_eq!(events[0].local_key_id, "system-gateway-key");
    assert_eq!(events[0].error_category.as_deref(), Some("codex_wake"));
}

#[tokio::test]
async fn account_wake_for_an_unknown_account_does_not_fallback_to_the_pool() {
    let (upstream, state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
    let authority = ready_authority("available-account", "available-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("fallback-source", &upstream, "source-key", -10)],
        vec![account(
            "available-account",
            "provider-available-account",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();

    let response = gateway::execute_account_wake(
        runtime,
        gateway::AccountWakeRequest {
            local_key_id: "system-gateway-key".into(),
            account_id: "missing-account".into(),
            model_id: MODEL.into(),
            output_token_cap: 8,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(state.requests.lock().unwrap().is_empty());
    assert!(events.lock().unwrap().is_empty());
}
