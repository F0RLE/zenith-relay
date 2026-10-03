use super::*;

#[tokio::test]
async fn model_identity_http_rotates_only_before_generated_output() {
    for generated in [false, true] {
        let reply = if generated {
            Reply::Json(
                StatusCode::OK,
                json!({"model": "different-model", "output": [{"type": "message", "content": [{"type": "output_text", "text": "already generated"}]}]}),
            )
        } else {
            Reply::Stream(vec![StreamChunk::Data("data: {\"type\":\"response.created\",\"response\":{\"model\":\"different-model\"}}\n\n")])
        };
        let (first, first_state) = spawn_upstream(vec![reply]).await;
        let (second, second_state) = spawn_upstream(vec![success_reply("correct-response")]).await;
        let authority = ready_authority("first", "synthetic-first").await;
        register_ready(&authority, "second", "synthetic-second").await;
        let (gateway, events, _, _) = spawn_mixed_gateway(
            Vec::new(),
            vec![
                account("first", "first", &first, 9000),
                account("second", "second", &second, 3000),
            ],
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
            .set_block_degraded_routes_enabled(true);
        let response = request(&gateway, false).await;
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!(first_state.requests.lock().unwrap().len(), 1);
        if generated {
            assert!(!status.is_success());
            assert_eq!(body["error"]["code"], "route_degraded");
            assert!(second_state.requests.lock().unwrap().is_empty());
        } else {
            assert_eq!(status, StatusCode::OK);
            assert_eq!(body["id"], "correct-response");
            assert_eq!(second_state.requests.lock().unwrap().len(), 1);
        }
        assert_eq!(
            events.lock().unwrap()[0].error_category.as_deref(),
            Some("upstream_route_degraded")
        );
    }
}

#[tokio::test]
async fn model_identity_websocket_rotates_before_output_and_rejects_reused_socket() {
    let (first, first_state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type": "response.created", "response": {"model": "different-model"}}),
        ])))
        .await;
    let (second, second_state) = spawn_websocket_upstream_with_behavior(WebSocketBehavior::Sequence(Arc::new(Mutex::new(VecDeque::from([
        vec![json!({"type": "response.completed", "response": {"id": "correct-response", "model": MODEL, "output": []}})],
        vec![json!({"type": "response.created", "response": {"model": "different-model", "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}}})],
    ]))))).await;
    let authority = ready_authority("first", "synthetic-first").await;
    register_ready(&authority, "second", "synthetic-second").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("first", "first", &first, 9000),
            account("second", "second", &second, 3000),
        ],
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
        .set_block_degraded_routes_enabled(true);
    let mut socket = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
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
            json!({"type": "response.create", "model": MODEL, "input": "first turn"}).to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "correct-response"
    );
    socket.send(ClientWsMessage::Text(json!({"type": "response.create", "model": MODEL, "previous_response_id": "correct-response", "input": "next turn"}).to_string())).await.unwrap();
    let failure = receive_websocket_json(&mut socket).await;
    assert_eq!(failure["error"]["code"], "route_degraded");
    assert_eq!(first_state.requests.lock().unwrap().len(), 1);
    assert_eq!(second_state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events[1].success);
    assert!(!events[2].success);
    assert_eq!(events[2].input_tokens, Some(3));
    assert_eq!(events[2].output_tokens, Some(2));
}
