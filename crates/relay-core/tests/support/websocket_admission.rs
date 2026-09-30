use super::*;

#[tokio::test]
async fn native_catalogs_details_and_generation_share_key_model_permissions() {
    let sources = [WireApi::Messages, WireApi::Gemini]
        .into_iter()
        .map(|wire_api| {
            RuntimeSource::unrestricted(ProviderSource {
                id: format!("catalog-{wire_api:?}"),
                name: "Synthetic catalog".into(),
                base_url: "http://127.0.0.1:1/v1".into(),
                api_key: SOURCE_KEY.into(),
                wire_api,
                models: vec!["shown".into(), "hidden".into()],
            })
        })
        .collect();
    let mut key = RuntimeLocalKey::unrestricted(LocalGatewayKey {
        id: "restricted".into(),
        secret: LOCAL_KEY.into(),
    });
    key.allowed_models = vec!["shown".into()];
    let runtime = GatewayRuntime::from_pool(
        sources,
        vec![key],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let gateway = spawn(gateway::router(Arc::new(runtime))).await;
    let client = reqwest::Client::new();
    for (base, header, list_field, id_field, expected_id, path, body) in [
        (
            "/v1/models",
            "x-api-key",
            "data",
            "id",
            "shown",
            "/v1/messages",
            json!({"model":"hidden","max_tokens":8,"messages":[{"role":"user","content":"test"}]}),
        ),
        (
            "/v1beta/models",
            "x-goog-api-key",
            "models",
            "name",
            "models/shown",
            "/v1beta/models/hidden:generateContent",
            json!({"contents":[{"role":"user","parts":[{"text":"test"}]}]}),
        ),
    ] {
        assert_eq!(
            client
                .get(format!("{}{base}", gateway.base_url))
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
        let list = client
            .get(format!("{}{base}", gateway.base_url))
            .header(header, LOCAL_KEY)
            .send()
            .await
            .unwrap();
        assert_eq!(list.status(), StatusCode::OK);
        let list: Value = list.json().await.unwrap();
        assert_eq!(list[list_field].as_array().unwrap().len(), 1);
        assert_eq!(list[list_field][0][id_field], expected_id);
        for (model, status) in [("shown", StatusCode::OK), ("hidden", StatusCode::NOT_FOUND)] {
            assert_eq!(
                client
                    .get(format!("{}{base}/{model}", gateway.base_url))
                    .header(header, LOCAL_KEY)
                    .send()
                    .await
                    .unwrap()
                    .status(),
                status
            );
        }
        assert_eq!(
            client
                .post(format!("{}{path}", gateway.base_url))
                .header(header, LOCAL_KEY)
                .json(&body)
                .send()
                .await
                .unwrap()
                .status(),
            StatusCode::NOT_FOUND
        );
    }
}

#[tokio::test]
async fn responses_websocket_falls_back_to_http_sse_upstream() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
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
            json!({
                "type": "response.create",
                "stream_id": "main_01-A.B",
                "model": "gpt-test",
                "input": "terminal-fragmented"
            })
            .to_string(),
        ))
        .await
        .unwrap();

    let mut saw_completed = false;
    for _ in 0..8 {
        let message = match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
            Ok(Some(Ok(message))) => message,
            _ => panic!("fallback did not produce a websocket event"),
        };
        let ClientWsMessage::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(text.as_ref()).unwrap();
        assert_eq!(
            value["stream_id"], "main_01-A.B",
            "named lane event was not scoped"
        );
        if value["type"] == "response.completed" {
            saw_completed = true;
            break;
        }
    }
    assert!(saw_completed, "HTTP/SSE upstream was not bridged to WS");
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert!(events.lock().unwrap().iter().any(|event| event.success));
}

#[tokio::test]
async fn responses_websocket_fallback_does_not_silently_accept_done_without_a_terminal() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, events) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
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
            json!({"type":"response.create","stream_id":"main","model":"gpt-test","input":"done-only-stream"}).to_string(),
        ))
        .await
        .unwrap();
    let event: Value = loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .expect("fallback must not leave the client waiting for a terminal")
            .expect("fallback must return an error event")
            .expect("WebSocket must be readable");
        if let ClientWsMessage::Text(text) = message {
            break serde_json::from_str(text.as_ref()).unwrap();
        }
    };
    assert_eq!(event["type"], "response.failed");
    assert_eq!(event["stream_id"], "main");
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("stream_incomplete")
    );
}

#[tokio::test]
async fn named_websocket_request_error_identifies_its_lane() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
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
                "stream_id": "planner",
                "model": "missing-model",
                "input": "hello"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let event: Value = loop {
        let message = tokio::time::timeout(Duration::from_secs(2), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        if let ClientWsMessage::Text(text) = message {
            break serde_json::from_str(text.as_ref()).unwrap();
        }
    };
    assert_eq!(event["type"], "error");
    assert_eq!(event["error"]["code"], "model_not_found");
    assert_eq!(event["stream_id"], "planner");
    assert!(state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn responses_rejects_unknown_previous_response_id_even_with_plaintext_history() {
    let state = UpstreamState::default();
    let upstream = spawn(
        Router::new()
            .route("/v1/responses", post(recording_upstream_responses))
            .with_state(state.clone()),
    )
    .await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": "resp_external_history",
            "input": [
                {"type":"message","role":"user","content":"What is the capital of France?"},
                {"type":"message","role":"assistant","content":"Paris is the capital of France."},
                {"type":"message","role":"user","content":"Name one landmark there."}
            ]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert!(state.bodies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn responses_rejects_unknown_opaque_previous_response_before_upstream_execution() {
    let state = UpstreamState::default();
    let upstream = spawn(
        Router::new()
            .route("/v1/responses", post(recording_upstream_responses))
            .with_state(state.clone()),
    )
    .await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": "gpt-test",
            "previous_response_id": "resp_external_opaque",
            "input": "continue"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "response_continuation_unavailable");
    assert!(state.requests.lock().unwrap().is_empty());
    assert!(state.bodies.lock().unwrap().is_empty());
}

#[tokio::test]
async fn responses_websocket_fallback_does_not_append_error_after_partial_output() {
    let (upstream, _) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
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
            json!({
                "type": "response.create",
                "model": "gpt-test",
                "input": "partial-truncated-stream"
            })
            .to_string(),
        ))
        .await
        .unwrap();

    let mut saw_partial = false;
    let mut saw_error = false;
    let mut saw_close = false;
    for _ in 0..8 {
        let message = match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
            Ok(Some(Ok(message))) => message,
            _ => break,
        };
        match message {
            ClientWsMessage::Text(text) => {
                let value: Value = serde_json::from_str(text.as_ref()).unwrap();
                saw_partial |= value["type"] == "response.output_text.delta";
                saw_error |= value["type"] == "error" || value["type"] == "response.failed";
            }
            ClientWsMessage::Close { .. } => {
                saw_close = true;
                break;
            }
            _ => {}
        }
    }
    assert!(saw_partial, "fallback did not forward the partial output");
    assert!(
        !saw_error,
        "fallback appended an error after partial output"
    );
    assert!(
        saw_close,
        "fallback did not close after an incomplete stream"
    );
}

#[tokio::test]
async fn responses_websocket_fallback_locks_the_first_later_stream_id() {
    let (upstream, state) = spawn_upstream().await;
    let (gateway, _) = spawn_gateway(&upstream.base_url, vec!["gpt-test"]).await;
    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();

    for payload in [
        json!({
            "type": "response.create",
            "model": "gpt-test",
            "input": "terminal-fragmented"
        }),
        json!({
            "type": "response.create",
            "stream_id": "stream-a",
            "model": "gpt-test",
            "input": "terminal-fragmented"
        }),
    ] {
        socket
            .send(ClientWsMessage::Text(payload.to_string()))
            .await
            .unwrap();

        let mut saw_completed = false;
        for _ in 0..8 {
            let message = match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
                Ok(Some(Ok(message))) => message,
                _ => panic!("fallback did not produce a websocket event"),
            };
            let ClientWsMessage::Text(text) = message else {
                continue;
            };
            let value: Value = serde_json::from_str(text.as_ref()).unwrap();
            if value["type"] == "response.completed" {
                saw_completed = true;
                break;
            }
        }
        assert!(saw_completed, "HTTP/SSE upstream was not bridged to WS");
    }

    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "stream_id": "stream-b",
                "model": "gpt-test",
                "input": "different-stream-id"
            })
            .to_string(),
        ))
        .await
        .unwrap();

    let mut rejection = None;
    for _ in 0..8 {
        let message = match tokio::time::timeout(Duration::from_secs(2), socket.next()).await {
            Ok(Some(Ok(message))) => message,
            _ => panic!("fallback did not reject the conflicting stream_id"),
        };
        let ClientWsMessage::Text(text) = message else {
            continue;
        };
        let value: Value = serde_json::from_str(text.as_ref()).unwrap();
        if value["type"] == "error" {
            rejection = Some(value);
            break;
        }
    }
    let rejection = rejection.expect("fallback did not emit an error event");
    assert_eq!(rejection["stream_id"], "stream-b");
    assert_eq!(rejection["error"]["code"], "invalid_request");
    assert!(rejection["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .contains("only one WebSocket stream_id is supported per connection"));
    assert_eq!(state.requests.lock().unwrap().len(), 2);
}
