use super::*;

#[tokio::test]
async fn codex_compatibility_aliases_reach_the_canonical_account_endpoints() {
    let (upstream, state) = spawn_upstream(vec![
        success_reply("alias-response"),
        Reply::Json(StatusCode::OK, json!({"type": "compaction"})),
        Reply::Json(StatusCode::OK, json!({"results": []})),
    ])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let response = client
        .post(format!(
            "{}/v1/chat/completions/v1/responses",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let compact = client
        .post(format!(
            "{}/v1/chat/completions/v1/responses/compact",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(compact.status(), StatusCode::OK);
    let search = client
        .post(format!(
            "{}/backend-api/codex/alpha/search",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "query": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::OK);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(requests[1].path, "/v1/responses/compact");
    assert_eq!(requests[2].path, "/v1/alpha/search");
}

#[tokio::test]
async fn account_only_endpoints_never_forward_to_an_api_key_source() {
    let (upstream, state) = spawn_upstream(vec![Reply::Json(
        StatusCode::OK,
        json!({
            "id": "resp_compact",
            "status": "completed",
            "output": [{
                "type": "compaction",
                "encrypted_content": "zenith-relay-compact-v1:test"
            }]
        }),
    )])
    .await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("api-source", &upstream, "source-secret", 100)],
        Vec::new(),
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(1).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let client = reqwest::Client::new();
    let search = client
        .post(format!("{}/v1/alpha/search", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "query": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(search.status(), StatusCode::NOT_FOUND);
    assert!(state.requests.lock().unwrap().is_empty());
    assert!(events.lock().unwrap().is_empty());

    let compact = client
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(compact.status(), StatusCode::OK);
    let body: Value = compact.json().await.unwrap();
    assert_eq!(body["object"], "response.compaction");
    assert_eq!(body["output"][0]["type"], "compaction");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/v1/responses");
    assert_eq!(requests[0].body["input"][1]["type"], "compaction_trigger");
}

#[tokio::test]
async fn previous_response_id_keeps_http_continuations_on_the_creating_account() {
    let (first_upstream, first_state) = spawn_upstream(vec![
        success_reply("first-response"),
        success_reply("first-continuation"),
    ])
    .await;
    let (second_upstream, second_state) = spawn_upstream(vec![
        success_reply("second-response"),
        success_reply("second-continuation"),
    ])
    .await;
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

    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "start"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let response_id = first["id"].as_str().unwrap();
    let continued = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "continue",
            "previous_response_id": response_id
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(continued.status(), StatusCode::OK);

    let counts = [
        first_state.requests.lock().unwrap().len(),
        second_state.requests.lock().unwrap().len(),
    ];
    assert!(counts == [2, 0] || counts == [0, 2]);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id, events[1].candidate_id);
}

#[tokio::test]
async fn model_switch_uses_materialized_http_history_before_selection() {
    let (old_model_upstream, old_state) =
        spawn_upstream(vec![success_reply("old-model-response")]).await;
    let (new_model_upstream, new_state) =
        spawn_upstream(vec![success_reply("new-model-response")]).await;
    let authority = Arc::new(TokenAuthority::new(2).unwrap());
    register_ready(&authority, "old-model-account", "old-model-access").await;
    register_ready(&authority, "new-model-account", "new-model-access").await;
    let mut old_model_account = account(
        "old-model-account",
        "provider-old-model",
        &old_model_upstream,
        100,
    );
    old_model_account.models = vec!["old-model".to_string()];
    let mut new_model_account = account(
        "new-model-account",
        "provider-new-model",
        &new_model_upstream,
        10,
    );
    new_model_account.models = vec!["new-model".to_string()];
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![old_model_account, new_model_account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": "old-model", "input": "start"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "old-model-response");

    let switched: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "new-model",
            "input": [
                {"type": "message", "role": "user", "content": "start"},
                {"type": "message", "role": "assistant", "content": "old-model response"},
                {"type": "message", "role": "user", "content": "continue with the new model"}
            ],
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(switched["id"], "new-model-response");
    assert_eq!(old_state.requests.lock().unwrap().len(), 1);
    let requests = new_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.get("previous_response_id").is_none());
    assert_eq!(requests[0].body["model"], "new-model");
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn http_continuation_materializes_the_full_chain_before_owner_rejection_fallback() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        success_reply("owner-first"),
        success_reply("owner-second"),
        Reply::Json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": {"code": "server_is_overloaded"}}),
        ),
    ])
    .await;
    let (backup_upstream, backup_state) = spawn_upstream(vec![
        success_reply("backup-third"),
        success_reply("backup-branch"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "backup-account", "backup-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account("backup-account", "provider-backup", &backup_upstream, 10),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(&gateway, &["owner-account", "backup-account"]);
    let client = reqwest::Client::new();

    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "first"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "second",
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let third: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "third",
            "previous_response_id": second["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(third["id"], "backup-third");
    assert!(gateway
        .runtime
        .as_ref()
        .unwrap()
        .set_candidate_health("owner-account", CandidateHealth::ReauthRequired,));
    let branch = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "alternate third",
            "previous_response_id": second["id"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(branch.status(), StatusCode::OK);
    assert_eq!(branch.json::<Value>().await.unwrap()["id"], "backup-branch");
    assert_eq!(owner_state.requests.lock().unwrap().len(), 3);
    let requests = backup_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0].body.get("previous_response_id").is_none());
    let input = requests[0].body["input"].as_array().unwrap();
    assert_eq!(input.len(), 3);
    for (item, text) in input.iter().zip(["first", "second", "third"]) {
        assert_eq!(item["role"], "user");
        assert_eq!(item["content"][0]["text"], text);
    }
    assert!(requests[1].body.get("previous_response_id").is_none());
    let branch_input = requests[1].body["input"].as_array().unwrap();
    assert_eq!(branch_input.len(), 3);
    for (item, text) in branch_input
        .iter()
        .zip(["first", "second", "alternate third"])
    {
        assert_eq!(item["content"][0]["text"], text);
    }
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 5);
    assert!(!events[2].success);
    assert!(events[3].success);
    assert_ne!(events[2].candidate_id, events[3].candidate_id);
}

#[tokio::test]
async fn http_continuation_replays_when_its_owner_leaves_the_active_pool() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![success_reply("owner-first")]).await;
    let (replacement_upstream, replacement_state) =
        spawn_upstream(vec![success_reply("replacement-second")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![
            source("owner-source", &owner_upstream, "owner-key", 100),
            source(
                "replacement-source",
                &replacement_upstream,
                "replacement-key",
                10,
            ),
        ],
        Vec::new(),
        vec![mixed_key(Some(vec!["owner-source"]), None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();
    let client = reqwest::Client::new();

    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "first"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "owner-first");

    assert!(runtime.update_key_scope(
        "local-key",
        CandidateScope {
            source_ids: Some(BTreeSet::from(["replacement-source".to_string()])),
            account_ids: Some(BTreeSet::new()),
            model_rules: Default::default(),
        },
    ));

    let continued: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "second",
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(continued["id"], "replacement-second");
    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
    let requests = replacement_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.get("previous_response_id").is_none());
    assert_eq!(requests[0].body["input"][0]["content"][0]["text"], "first");
    assert_eq!(requests[0].body["input"][1]["content"][0]["text"], "second");
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
    assert_ne!(events[0].candidate_id, events[1].candidate_id);
}

#[tokio::test]
async fn http_continuation_replays_to_api_source_after_account_removal() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![success_reply("owner-first")]).await;
    let (replacement_upstream, replacement_state) =
        spawn_upstream(vec![success_reply("replacement-second")]).await;
    let authority = ready_authority("owner-account", "owner-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source(
            "replacement-source",
            &replacement_upstream,
            "replacement-key",
            10,
        )],
        vec![account(
            "owner-account",
            "provider-owner",
            &owner_upstream,
            100,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();
    let client = reqwest::Client::new();

    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "first"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "owner-first");
    assert!(runtime.remove_candidate("owner-account"));

    let continued: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "second",
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(continued["id"], "replacement-second");
    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
    let requests = replacement_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].body.get("previous_response_id").is_none());
    assert_eq!(requests[0].body["input"][0]["content"][0]["text"], "first");
    assert_eq!(requests[0].body["input"][1]["content"][0]["text"], "second");
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn api_pool_removal_releases_only_self_contained_tool_continuations() {
    for stream in [false, true] {
        for include_call in [true, false] {
            let call = json!({
                "type": "function_call", "id": "fc_rotate", "call_id": "call_rotate",
                "name": "lookup", "arguments": "{}"
            });
            let (owner, owner_state) = spawn_upstream(vec![Reply::Json(
                StatusCode::OK,
                json!({"id": "api-tool-response", "model": MODEL, "output": [call.clone()]}),
            )])
            .await;
            let (replacement, replacement_state) = spawn_upstream(vec![if stream {
                successful_sse_reply()
            } else {
                success_reply("api-replacement-response")
            }])
            .await;
            let (gateway, _, _, _) = spawn_mixed_gateway(
                vec![
                    source("api-owner", &owner, "owner-key", 100),
                    source("api-replacement", &replacement, "replacement-key", 10),
                ],
                Vec::new(),
                vec![mixed_key(Some(vec!["api-owner"]), Some(Vec::new()))],
                Arc::new(TokenAuthority::new(4).unwrap()),
                refresh_adapter(),
                Arc::new(PersistenceAdapter::default()),
            )
            .await;
            let first = request(&gateway, false).await;
            assert_eq!(first.status(), StatusCode::OK);
            let _: Value = first.json().await.unwrap();
            assert!(gateway.runtime.as_ref().unwrap().update_key_scope(
                "local-key",
                CandidateScope {
                    source_ids: Some(BTreeSet::from(["api-replacement".to_string()])),
                    account_ids: Some(BTreeSet::new()),
                    model_rules: Default::default(),
                },
            ));

            let mut input = Vec::new();
            if include_call {
                input.push(call);
            }
            input.push(json!({
                "type": "function_call_output", "call_id": "call_rotate", "output": "synthetic result"
            }));
            let response = reqwest::Client::new()
                .post(format!("{}/v1/responses", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .json(&json!({"model": MODEL, "stream": stream, "input": input}))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let _ = response.bytes().await.unwrap();
            assert_eq!(
                status.is_success(),
                include_call,
                "stream={stream}, paired={include_call}, status={status}"
            );
            assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
            let requests = replacement_state.requests.lock().unwrap();
            assert_eq!(requests.len(), usize::from(include_call));
            if include_call {
                assert_eq!(requests[0].body["input"], json!(input));
                assert!(requests[0].body.get("previous_response_id").is_none());
            }
        }
    }
}

#[tokio::test]
async fn websocket_api_pool_removal_preserves_self_contained_tool_history() {
    for reconnect in [false, true] {
        let call = json!({
            "type": "function_call", "id": "fc_rotate", "call_id": "call_rotate",
            "name": "lookup", "arguments": "{}"
        });
        let (owner, owner_state) =
            spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
                json!({"type": "response.output_item.done", "item": call.clone()}),
                json!({"type": "response.completed", "response": {
                    "id": "api-tool-response", "status": "completed", "model": MODEL,
                    "output": [call.clone()]
                }}),
            ])))
            .await;
        let (replacement, replacement_state) = spawn_websocket_upstream().await;
        let (gateway, _, _, _) = spawn_mixed_gateway(
            vec![
                source("api-owner", &owner, "owner-key", 100),
                source("api-replacement", &replacement, "replacement-key", 10),
            ],
            Vec::new(),
            vec![mixed_key(Some(vec!["api-owner"]), Some(Vec::new()))],
            Arc::new(TokenAuthority::new(4).unwrap()),
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let client = reqwest::Client::new();
        let url = format!("{}/v1/responses", gateway.base_url);
        let mut socket = client
            .get(&url)
            .bearer_auth(LOCAL_KEY)
            .upgrade()
            .send()
            .await
            .unwrap()
            .into_websocket()
            .await
            .unwrap();
        socket
            .send(ClientWsMessage::Text(
                json!({
                    "type": "response.create", "model": MODEL, "input": "synthetic lookup"
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let first = receive_websocket_completion(&mut socket).await;
        assert_eq!(first["response"]["id"], "api-tool-response");

        assert!(gateway.runtime.as_ref().unwrap().update_key_scope(
            "local-key",
            CandidateScope {
                source_ids: Some(BTreeSet::from(["api-replacement".to_string()])),
                account_ids: Some(BTreeSet::new()),
                model_rules: Default::default(),
            },
        ));
        if reconnect {
            drop(socket);
            socket = client
                .get(&url)
                .bearer_auth(LOCAL_KEY)
                .upgrade()
                .send()
                .await
                .unwrap()
                .into_websocket()
                .await
                .unwrap();
        }
        let input = json!([
            call,
            {"type": "function_call_output", "call_id": "call_rotate", "output": "synthetic result"}
        ]);
        socket
            .send(ClientWsMessage::Text(
                json!({
                    "type": "response.create", "model": MODEL, "input": input
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let _ = receive_websocket_completion(&mut socket).await;
        assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
        let requests = replacement_state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0]["input"], input);
        assert!(requests[0].get("previous_response_id").is_none());
    }
}

#[tokio::test]
async fn prompt_cache_key_keeps_sequential_http_requests_on_the_same_account() {
    let (first_upstream, first_state) = spawn_upstream(vec![
        success_reply("first-response"),
        success_reply("first-continuation"),
    ])
    .await;
    let (second_upstream, second_state) = spawn_upstream(vec![
        success_reply("second-response"),
        success_reply("second-continuation"),
    ])
    .await;
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

    let client = reqwest::Client::new();
    for input in ["start", "continue"] {
        let response = client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({
                "model": MODEL,
                "input": input,
                "prompt_cache_key": "thread-1"
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let counts = [
        first_state.requests.lock().unwrap().len(),
        second_state.requests.lock().unwrap().len(),
    ];
    assert!(counts == [2, 0] || counts == [0, 2]);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id, events[1].candidate_id);
    assert_eq!(
        events[1].routing.as_ref().map(|routing| routing.reason),
        Some(SelectionReason::PromptCacheAffinity)
    );
}

fn fresh_quota(remaining: u16, observed_at_ms: u64) -> zenith_relay_core::quota::QuotaSnapshot {
    zenith_relay_core::quota::QuotaSnapshot {
        primary: Some(zenith_relay_core::quota::QuotaWindow {
            kind: zenith_relay_core::quota::QuotaWindowKind::Primary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(remaining),
            explicitly_full: None,
            reset_at_ms: Some(observed_at_ms.saturating_add(3_600_000)),
            window_minutes: Some(300),
            observed_at_ms,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }),
        updated_at_ms: Some(observed_at_ms),
        ..Default::default()
    }
}

fn set_account_remainder(runtime: &GatewayRuntime, account_id: &str, remaining: u16) {
    let observed_at_ms = current_time_ms();
    assert!(runtime.sync_account_availability_with_quota(
        account_id,
        true,
        CandidateHealth::Healthy,
        &fresh_quota(remaining, observed_at_ms),
        observed_at_ms,
    ));
}

fn reply_without_saved_history(id: &str) -> Reply {
    Reply::Json(
        StatusCode::OK,
        json!({
            "id": id,
            "object": "response",
            "model": MODEL,
            "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
        }),
    )
}

#[tokio::test]
async fn account_compact_continuation_moves_only_for_a_larger_fresh_remainder() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        success_reply("owner-compact"),
        success_reply("owner-stays"),
        success_reply("owner-after-shrink"),
    ])
    .await;
    let (other_upstream, other_state) = spawn_upstream(vec![success_reply("other-compact")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "other-account", "other-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account("other-account", "provider-other", &other_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();
    set_account_remainder(&runtime, "owner-account", 9_800);
    set_account_remainder(&runtime, "other-account", 3_300);
    let client = reqwest::Client::new();

    let first: Value = client
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "first"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "owner-compact");

    let stayed: Value = client
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "second",
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stayed["id"], "owner-stays");
    assert_eq!(
        owner_state.requests.lock().unwrap()[1]
            .body
            .get("previous_response_id")
            .and_then(Value::as_str),
        Some("owner-compact")
    );
    assert!(other_state.requests.lock().unwrap().is_empty());

    set_account_remainder(&runtime, "owner-account", 3_200);
    set_account_remainder(&runtime, "other-account", 9_800);
    let moved = client
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "third",
            "previous_response_id": stayed["id"]
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(moved.status(), StatusCode::OK);
    let moved: Value = moved.json().await.unwrap();
    assert_eq!(moved["id"], "other-compact");
    assert_eq!(owner_state.requests.lock().unwrap().len(), 2);
    let other_requests = other_state.requests.lock().unwrap();
    assert_eq!(other_requests.len(), 1);
    assert_eq!(other_requests[0].path, "/v1/responses/compact");
    assert_eq!(
        other_requests[0].chatgpt_account_id.as_deref(),
        Some("provider-other")
    );
    assert!(other_requests[0].body.get("previous_response_id").is_none());
    assert_eq!(
        other_requests[0].body["input"][0]["content"][0]["text"],
        "first"
    );
    assert_eq!(
        other_requests[0].body["input"][1]["content"][0]["text"],
        "second"
    );
    assert_eq!(
        other_requests[0].body["input"][2]["content"][0]["text"],
        "third"
    );
}

#[tokio::test]
async fn account_continuation_without_saved_history_stays_on_its_owner() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        reply_without_saved_history("owner-opaque"),
        success_reply("owner-follow-up"),
    ])
    .await;
    let (other_upstream, other_state) = spawn_upstream(vec![success_reply("other-unused")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "other-account", "other-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account("other-account", "provider-other", &other_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();
    set_account_remainder(&runtime, "owner-account", 9_800);
    set_account_remainder(&runtime, "other-account", 3_300);
    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "first"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "owner-opaque");

    set_account_remainder(&runtime, "owner-account", 3_200);
    set_account_remainder(&runtime, "other-account", 9_800);
    let stayed: Value = client
        .post(format!("{}/v1/alpha/search", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "second",
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stayed["id"], "owner-follow-up");
    let owner_requests = owner_state.requests.lock().unwrap();
    assert_eq!(owner_requests.len(), 2);
    assert_eq!(owner_requests[1].path, "/v1/alpha/search");
    assert_eq!(
        owner_requests[1]
            .body
            .get("previous_response_id")
            .and_then(Value::as_str),
        Some("owner-opaque")
    );
    drop(owner_requests);
    assert!(other_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn account_search_continuation_moves_when_another_account_has_more_quota() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![success_reply("owner-response")]).await;
    let (other_upstream, other_state) = spawn_upstream(vec![success_reply("other-search")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "other-account", "other-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account("other-account", "provider-other", &other_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();
    set_account_remainder(&runtime, "owner-account", 9_800);
    set_account_remainder(&runtime, "other-account", 3_300);
    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model": MODEL, "input": "first", "stream": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "owner-response");

    set_account_remainder(&runtime, "owner-account", 3_200);
    set_account_remainder(&runtime, "other-account", 9_800);
    let moved: Value = client
        .post(format!("{}/v1/alpha/search", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "second",
            "previous_response_id": first["id"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(moved["id"], "other-search");
    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
    let other_requests = other_state.requests.lock().unwrap();
    assert_eq!(other_requests.len(), 1);
    assert_eq!(other_requests[0].path, "/v1/alpha/search");
    assert!(other_requests[0].body.get("previous_response_id").is_none());
    assert_eq!(
        other_requests[0].body["input"][0]["content"][0]["text"],
        "first"
    );
    assert_eq!(
        other_requests[0].body["input"][1]["content"][0]["text"],
        "second"
    );
}

#[tokio::test]
async fn account_wake_stays_on_its_account_when_another_has_more_quota() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![success_reply("wake-response")]).await;
    let (other_upstream, other_state) = spawn_upstream(vec![success_reply("other-unused")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "other-account", "other-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account("other-account", "provider-other", &other_upstream, 100),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();
    set_account_remainder(&runtime, "owner-account", 3_200);
    set_account_remainder(&runtime, "other-account", 9_800);

    let response = gateway::execute_account_wake(
        runtime,
        gateway::AccountWakeRequest {
            local_key_id: "system-gateway-key".into(),
            account_id: "owner-account".into(),
            model_id: MODEL.into(),
            output_token_cap: 8,
        },
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let owner_requests = owner_state.requests.lock().unwrap();
    assert_eq!(owner_requests.len(), 1);
    assert_eq!(
        owner_requests[0].chatgpt_account_id.as_deref(),
        Some("provider-owner")
    );
    drop(owner_requests);
    assert!(other_state.requests.lock().unwrap().is_empty());
}
