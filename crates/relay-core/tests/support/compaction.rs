use super::*;

fn compacted_input() -> Value {
    json!([
        {"role":"user","content":"Retained user context"},
        {"id":"cmp_test","type":"compaction","encrypted_content":"synthetic-checkpoint"},
        {"type":"function_call","call_id":"call_test","name":"lookup","arguments":"{}"},
        {"type":"function_call_output","call_id":"call_test","output":"synthetic result"},
        {"role":"user","content":"Continue the task"}
    ])
}

#[tokio::test]
async fn compacted_history_resets_stale_ids_for_http_and_account_compact() {
    for (path, api_source) in [
        ("/v1/responses", false),
        ("/v1/responses/compact", false),
        ("/v1/responses", true),
    ] {
        let (upstream, state) = spawn_upstream(vec![success_reply("compacted-response")]).await;
        let authority = ready_authority("compact-account", "synthetic-access").await;
        let (sources, accounts) = if api_source {
            (
                vec![source("compact-source", &upstream, "synthetic-key", 10)],
                vec![],
            )
        } else {
            (
                vec![],
                vec![account(
                    "compact-account",
                    "synthetic-provider",
                    &upstream,
                    10,
                )],
            )
        };
        let (gateway, _, _, _) = spawn_mixed_gateway(
            sources,
            accounts,
            vec![mixed_key(None, None)],
            authority,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let input = compacted_input();
        let response = reqwest::Client::new()
            .post(format!("{}{path}", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({
                "model":MODEL,"previous_response_id":"resp_from_old_runtime",
                "input":input,
                "context_management":[{"type":"compaction","compact_threshold":1000}]
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{path}, api source: {api_source}"
        );
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].body.get("previous_response_id").is_none());
        assert_eq!(requests[0].body["input"], input);
        assert_eq!(
            requests[0].body["context_management"][0]["compact_threshold"],
            1000
        );
    }
}

#[tokio::test]
async fn compacted_history_replaces_the_window_on_a_live_websocket() {
    let (upstream, state) = spawn_websocket_upstream().await;
    let authority = ready_authority("compact-account", "synthetic-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "compact-account",
            "synthetic-provider",
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
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type":"response.create","model":MODEL,"input":"Initial context before compaction"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let first = receive_websocket_completion(&mut socket).await;
    let owned_id = first["response"]["id"].as_str().unwrap();
    let input = compacted_input();
    for previous_id in [Some(owned_id), Some("resp_from_old_runtime"), None] {
        let mut request = json!({"type":"response.create","model":MODEL,"input":input});
        if let Some(id) = previous_id {
            request["previous_response_id"] = json!(id);
        }
        socket
            .send(ClientWsMessage::Text(request.to_string()))
            .await
            .unwrap();
        let completed = receive_websocket_completion(&mut socket).await;
        assert_eq!(completed["type"], "response.completed");
    }
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 4);
    for request in requests.iter().skip(1) {
        assert!(request.get("previous_response_id").is_none());
        assert_eq!(request["input"], input);
    }
}

#[tokio::test]
async fn compacted_history_with_missing_tool_context_is_rejected_before_dispatch() {
    let (upstream, state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
    let authority = ready_authority("compact-account", "synthetic-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "compact-account",
            "synthetic-provider",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let mut input = compacted_input();
    input.as_array_mut().unwrap().remove(2);
    for path in ["/v1/responses", "/v1/responses/compact"] {
        let response = reqwest::Client::new()
            .post(format!("{}{path}", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":MODEL,"previous_response_id":"resp_missing","input":input}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }
    assert!(state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn compacted_history_is_not_stripped_after_websocket_ciphertext_rejection() {
    let (upstream, state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![json!({
            "type":"error","status":400,"error":{"code":"invalid_encrypted_content"}
        })])))
        .await;
    let authority = ready_authority("compact-account", "synthetic-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "compact-account",
            "synthetic-provider",
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
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut socket = upgraded.into_websocket().await.unwrap();
    let input = compacted_input();
    socket
        .send(ClientWsMessage::Text(
            json!({
                "type":"response.create","model":MODEL,"input":input
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let failure = receive_websocket_json(&mut socket).await;
    assert_eq!(failure["type"], "error");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["input"], input);
}
