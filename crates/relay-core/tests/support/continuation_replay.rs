use super::*;

#[tokio::test]
async fn account_websocket_keeps_previous_response_on_its_current_account() {
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
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "start"}).to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let completed = receive_websocket_completion(&mut socket).await;
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": [{
                    "type": "function_call_output",
                    "call_id": "call_live",
                    "output": "done"
                }],
                "previous_response_id": completed["response"]["id"]
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let _ = receive_websocket_completion(&mut socket).await;
    tokio::time::sleep(Duration::from_millis(10)).await;

    let counts = [
        first_state.requests.lock().unwrap().len(),
        second_state.requests.lock().unwrap().len(),
    ];
    assert!(counts == [2, 0] || counts == [0, 2]);
    assert_eq!(
        first_state.headers.lock().unwrap().len() + second_state.headers.lock().unwrap().len(),
        1
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id, events[1].candidate_id);
}

#[tokio::test]
async fn account_websocket_restores_previous_response_affinity_after_reconnect() {
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

    let client = reqwest::Client::new();
    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "start"}).to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let completed = receive_websocket_completion(&mut socket).await;
    let response_id = completed["response"]["id"].as_str().unwrap().to_string();
    drop(socket);

    let upgraded = client
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
                "input": "continue after reconnect",
                "previous_response_id": response_id
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let _ = receive_websocket_completion(&mut socket).await;
    tokio::time::sleep(Duration::from_millis(10)).await;

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
async fn websocket_continuation_replays_to_another_candidate_after_owner_quota() {
    let reset_at = current_time_ms() / 1_000 + 60 * 60;
    let owner_events = Arc::new(Mutex::new(VecDeque::from(vec![
        vec![
            json!({"type": "response.output_text.delta", "delta": "first"}),
            json!({
                "type": "response.completed",
                "response": {"id": "owner-response", "output": [{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"first"}]}]}
            }),
        ],
        vec![json!({
            "type": "response.failed",
            "response": {
                "error": {
                    "type": "usage_limit_reached",
                    "message": "Usage limit reached",
                    "resets_at": reset_at
                }
            }
        })],
    ])));
    let (owner_upstream, owner_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Sequence(owner_events)).await;
    let (replacement_upstream, replacement_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Success).await;
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
        vec![mixed_key(None, None)],
        Arc::new(TokenAuthority::new(1).unwrap()),
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let client = reqwest::Client::new();
    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "first"}).to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let completed = receive_websocket_completion(&mut socket).await;
    let response_id = completed["response"]["id"].as_str().unwrap().to_string();
    drop(socket);

    let upgraded = client
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
                "input": "second",
                "previous_response_id": response_id
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let completed = receive_websocket_completion(&mut socket).await;
    assert_eq!(completed["response"]["id"], "ws-response");

    let owner_requests = owner_state.requests.lock().unwrap();
    assert_eq!(owner_requests.len(), 2);
    drop(owner_requests);
    let replacement_requests = replacement_state.requests.lock().unwrap();
    assert_eq!(replacement_requests.len(), 1);
    assert!(replacement_requests[0]
        .get("previous_response_id")
        .is_none());
    assert_eq!(
        replacement_requests[0]["input"][0]["content"][0]["text"],
        "first"
    );
    assert_eq!(replacement_requests[0]["input"][1]["role"], "assistant");
    assert_eq!(
        replacement_requests[0]["input"][1]["content"][0]["text"],
        "first"
    );
    assert_eq!(
        replacement_requests[0]["input"][2]["content"][0]["text"],
        "second"
    );
    drop(replacement_requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_quota_exhausted")
    );
    assert!(events[2].success);
    assert_eq!(
        events[2].candidate_id.as_deref(),
        Some("replacement-source")
    );
}

#[tokio::test]
async fn websocket_continuation_replays_on_the_same_connection_when_another_chat_exhausts_owner() {
    let (owner_upstream, owner_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type": "response.output_text.delta", "delta": "first"}),
            json!({
                "type": "response.completed",
                "response": {"id": "owner-response", "output": [{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"first"}]}]}
            }),
        ])))
        .await;
    let (replacement_upstream, replacement_state) = spawn_websocket_upstream().await;
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
            json!({"type": "response.create", "model": MODEL, "input": "first"}).to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let first = receive_websocket_completion(&mut socket).await;
    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);

    // Another chat exhausts the owner while this client WebSocket remains open.
    // The next continuation must materialize local history and reconnect to a
    // compatible API source instead of returning an affinity-owner 429.
    let exhausted_at = current_time_ms().saturating_add(1);
    assert!(runtime.sync_account_availability_with_quota(
        "owner-account",
        true,
        CandidateHealth::Healthy,
        &zenith_relay_core::quota::QuotaSnapshot {
            limit_reached: true,
            updated_at_ms: Some(exhausted_at),
            ..Default::default()
        },
        exhausted_at,
    ));

    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "second",
                "previous_response_id": first["response"]["id"]
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let _ = receive_websocket_completion(&mut socket).await;

    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
    let replacement_requests = replacement_state.requests.lock().unwrap();
    assert_eq!(replacement_requests.len(), 1);
    assert!(replacement_requests[0]
        .get("previous_response_id")
        .is_none());
    assert_eq!(
        replacement_requests[0]["input"][0]["content"][0]["text"],
        "first"
    );
    assert_eq!(replacement_requests[0]["input"][1]["role"], "assistant");
    assert_eq!(
        replacement_requests[0]["input"][1]["content"][0]["text"],
        "first"
    );
    assert_eq!(
        replacement_requests[0]["input"][2]["content"][0]["text"],
        "second"
    );
    drop(replacement_requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
    assert_eq!(
        events[1].candidate_id.as_deref(),
        Some("replacement-source")
    );
}

#[tokio::test]
async fn http_continuation_replays_to_api_source_after_account_quota() {
    let reset_at = current_time_ms() / 1_000 + 60 * 60;
    let (owner_upstream, owner_state) = spawn_upstream(vec![
        success_reply("owner-response"),
        Reply::Json(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": {
                "message": "exceeded retry limit, last status: 429 Too Many Requests",
                "resets_at": reset_at
            }}),
        ),
    ])
    .await;
    let (replacement_upstream, replacement_state) =
        spawn_upstream(vec![success_reply("replacement-response")]).await;
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

    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "first",
            "stream": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "owner-response");
    let continued: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "second",
            "previous_response_id": first["id"],
            "stream": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(continued["id"], "replacement-response");
    assert_eq!(owner_state.requests.lock().unwrap().len(), 2);
    let replacement_requests = replacement_state.requests.lock().unwrap();
    assert_eq!(replacement_requests.len(), 1);
    assert!(replacement_requests[0]
        .body
        .get("previous_response_id")
        .is_none());
    assert_eq!(
        replacement_requests[0].body["input"][0]["content"][0]["text"],
        "first"
    );
    assert_eq!(
        replacement_requests[0].body["input"][1]["content"][0]["text"],
        "second"
    );
    drop(replacement_requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("upstream_rate_limited")
    );
    assert!(events[2].success);
    assert_eq!(
        events[2].candidate_id.as_deref(),
        Some("replacement-source")
    );
}

#[tokio::test]
async fn http_continuation_replays_to_api_source_when_another_chat_exhausted_the_owner() {
    let (owner_upstream, owner_state) = spawn_upstream(vec![success_reply("owner-response")]).await;
    let (replacement_upstream, replacement_state) =
        spawn_upstream(vec![success_reply("replacement-response")]).await;
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
        .json(&json!({"model": MODEL, "input": "first", "stream": false}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(first["id"], "owner-response");

    // A different chat can consume this account's quota before the original
    // conversation sends its next turn. The affinity owner is then ineligible
    // before selection, so Replay must hand the full local conversation to the
    // next compatible source without retrying the exhausted account.
    let exhausted_at = current_time_ms().saturating_add(1);
    assert!(runtime.sync_account_availability_with_quota(
        "owner-account",
        true,
        CandidateHealth::Healthy,
        &zenith_relay_core::quota::QuotaSnapshot {
            limit_reached: true,
            updated_at_ms: Some(exhausted_at),
            ..Default::default()
        },
        exhausted_at,
    ));

    let continued: Value = client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": "second",
            "previous_response_id": first["id"],
            "stream": false
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(continued["id"], "replacement-response");
    assert_eq!(owner_state.requests.lock().unwrap().len(), 1);
    let replacement_requests = replacement_state.requests.lock().unwrap();
    assert_eq!(replacement_requests.len(), 1);
    assert!(replacement_requests[0]
        .body
        .get("previous_response_id")
        .is_none());
    assert_eq!(
        replacement_requests[0].body["input"][0]["content"][0]["text"],
        "first"
    );
    assert_eq!(
        replacement_requests[0].body["input"][1]["content"][0]["text"],
        "second"
    );
    drop(replacement_requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
    assert_eq!(
        events[1].candidate_id.as_deref(),
        Some("replacement-source")
    );
}

#[tokio::test]
async fn prompt_cache_key_keeps_reconnected_websocket_on_the_same_account() {
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

    let client = reqwest::Client::new();
    for input in ["start", "continue"] {
        let upgraded = client
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
                    "input": input,
                    "prompt_cache_key": "thread-1"
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let _ = receive_websocket_json(&mut socket).await;
        let _ = receive_websocket_completion(&mut socket).await;
    }
    tokio::time::sleep(Duration::from_millis(10)).await;

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

#[tokio::test]
async fn unknown_websocket_response_owner_is_rejected_before_candidate_selection() {
    let (wrong_upstream, wrong_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type": "error",
            "status": 400,
            "error": {
                "message": "Previous response with id 'response-from-before-restart' not found.",
                "type": "invalid_request_error",
                "code": "previous_response_not_found"
            }
        })])))
        .await;
    let (owner_upstream, owner_state) = spawn_websocket_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "wrong-account", "wrong-access").await;
    register_ready(&authority, "owner-account", "owner-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("wrong-account", "provider-wrong", &wrong_upstream, 100),
            account("owner-account", "provider-owner", &owner_upstream, 10),
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
                "type": "response.create",
                "model": MODEL,
                "input": "continue after restart",
                "previous_response_id": "response-from-before-restart"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["error"]["code"], "response_continuation_unavailable");

    assert!(wrong_state.requests.lock().unwrap().is_empty());
    assert!(owner_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn orphaned_websocket_response_is_rejected_without_materialized_history() {
    let (missing_upstream, missing_state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::Sequence(Arc::new(Mutex::new(VecDeque::from(vec![
            vec![json!({
                "type": "error",
                "status": 400,
                "error": {
                    "message": "Previous response with id 'orphaned-response' not found.",
                    "type": "invalid_request_error",
                    "code": "previous_response_not_found"
                }
            })],
            vec![
                json!({"type": "response.output_text.delta", "delta": "fresh"}),
                json!({
                    "type": "response.completed",
                    "response": {"id": "fresh-ws-response"}
                }),
            ],
        ])))),
    )
    .await;
    let (transport_upstream, transport_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type": "error",
            "status": 502,
            "error": {"code": "upstream_transport", "message": "temporary transport failure"}
        })])))
        .await;
    let (limited_upstream, limited_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type": "error",
            "status": 429,
            "error": {"code": "rate_limit_exceeded", "message": "rate limited"}
        })])))
        .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "missing-account", "missing-access").await;
    register_ready(&authority, "transport-account", "transport-access").await;
    register_ready(&authority, "limited-account", "limited-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "missing-account",
                "provider-missing",
                &missing_upstream,
                100,
            ),
            account(
                "transport-account",
                "provider-transport",
                &transport_upstream,
                50,
            ),
            account("limited-account", "provider-limited", &limited_upstream, 10),
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
                "type": "response.create",
                "model": MODEL,
                "input": "continue without an available response owner",
                "previous_response_id": "orphaned-response"
            })
            .to_string(),
        ))
        .await
        .unwrap();

    let error = receive_websocket_json(&mut socket).await;
    assert_eq!(error["type"], "error");
    assert_eq!(error["error"]["code"], "response_continuation_unavailable");
    assert!(missing_state.requests.lock().unwrap().is_empty());
    assert!(transport_state.requests.lock().unwrap().is_empty());
    assert!(limited_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    assert!(events.is_empty());
}

#[tokio::test]
async fn stale_websocket_response_affinity_resets_before_quota_routing() {
    assert_stale_websocket_continuation(true).await;
}

#[tokio::test]
async fn stale_websocket_response_replays_on_the_live_connection() {
    assert_stale_websocket_continuation(false).await;
}

async fn assert_stale_websocket_continuation(reconnect: bool) {
    let (fallback_upstream, fallback_state) = spawn_websocket_upstream().await;
    let (owner_upstream, owner_state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::Sequence(Arc::new(Mutex::new(VecDeque::from(vec![
            vec![
                json!({"type": "response.output_text.delta", "delta": "owner"}),
                json!({"type": "response.completed", "response": {"id": "stale-ws-response", "output":[{"type":"message","role":"assistant","phase":"final_answer","content":[{"type":"output_text","text":"owner"}]}]}}),
            ],
            vec![json!({
                "type": "error",
                "status": 400,
                "error": {
                    "message": "Previous response with id 'stale-ws-response' not found.",
                    "type": "invalid_request_error",
                    "code": "previous_response_not_found"
                }
            })],
            vec![
                json!({"type": "response.output_text.delta", "delta": "recovered"}),
                json!({"type": "response.completed", "response": {"id": "recovered-ws-response"}}),
            ],
        ])))),
    )
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "owner-account", "owner-access").await;
    register_ready(&authority, "fallback-account", "fallback-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("owner-account", "provider-owner", &owner_upstream, 100),
            account(
                "fallback-account",
                "provider-fallback",
                &fallback_upstream,
                10,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(&gateway, &["owner-account", "fallback-account"]);

    let client = reqwest::Client::new();
    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut first_socket = upgraded.into_websocket().await.unwrap();
    first_socket
        .send(ClientWsMessage::Text(
            json!({"type": "response.create", "model": MODEL, "input": "start"}).to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut first_socket).await;
    let first_completed = receive_websocket_completion(&mut first_socket).await;
    let response_id = first_completed["response"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let mut second_socket = if reconnect {
        drop(first_socket);
        let upgraded = client
            .get(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .upgrade()
            .send()
            .await
            .unwrap();
        upgraded.into_websocket().await.unwrap()
    } else {
        first_socket
    };
    second_socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "continue after stale binding",
                "previous_response_id": response_id
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_json(&mut second_socket).await["delta"],
        "recovered"
    );
    assert_eq!(
        receive_websocket_completion(&mut second_socket).await["response"]["id"],
        "recovered-ws-response"
    );

    assert!(gateway
        .runtime
        .as_ref()
        .unwrap()
        .set_candidate_health("owner-account", CandidateHealth::ReauthRequired,));
    second_socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "branch from original",
                "previous_response_id": response_id
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_completion(&mut second_socket).await["type"],
        "response.completed"
    );

    let requests = owner_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[2].get("previous_response_id").is_none());
    assert_eq!(requests[2]["input"][1]["phase"], "final_answer");
    assert_eq!(requests[2]["input"][1]["content"][0]["text"], "owner");
    drop(requests);
    let requests = fallback_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].get("previous_response_id").is_none());
    assert_eq!(requests[0]["input"].as_array().unwrap().len(), 3);
    assert_eq!(requests[0]["input"][0]["content"][0]["text"], "start");
    assert_eq!(requests[0]["input"][1]["content"][0]["text"], "owner");
    assert_eq!(
        requests[0]["input"][2]["content"][0]["text"],
        "branch from original"
    );
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 4);
    assert_eq!(
        events[1].error_category.as_deref(),
        Some("response_affinity_miss")
    );
    assert!(events[2].success);
}
