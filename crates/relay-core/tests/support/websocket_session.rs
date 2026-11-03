use super::*;

#[tokio::test]
async fn guarded_websocket_reuses_decoding_without_changing_text_or_binary_frames() {
    let frames = Arc::new(vec![
        AxumWsMessage::Text(r#"{ "type":"response.output_text.delta", "output_index":0, "delta":"hello\n\"quoted\"" }"#.into()),
        AxumWsMessage::Binary(br#"{ "type":"response.function_call_arguments.delta", "output_index":1, "delta":"{" }"#.as_slice().into()),
        AxumWsMessage::Text(r#"{"type":"response.in_progress","response":{"id":"ws-bytes","usage":{"input_tokens":11,"input_tokens_details":{"cached_tokens":7}}}}"#.into()),
        AxumWsMessage::Text(r#"{"type":"responsesapi.websocket_timing","response_id":"old-response"}"#.into()),
        AxumWsMessage::Text(r#"{ "type":"response.future_event", "extension":123456789012345678901234567890 }"#.into()),
        AxumWsMessage::Binary(vec![0xff, 0xfe].into()),
        AxumWsMessage::Text(r#"{ "type":"response.completed", "response":{"id":"ws-bytes","status":"completed","output":[],"usage":{"output_tokens":5}} }"#.into()),
    ]);
    let (upstream, state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Frames(frames.clone())).await;
    let authority = ready_authority("bytes-account", "synthetic-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("bytes-account", "provider-bytes", &upstream, 10)],
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
    for _ in 0..2 {
        socket
            .send(ClientWsMessage::Text(
                json!({"type":"response.create","model":MODEL,"input":"synthetic request"})
                    .to_string(),
            ))
            .await
            .unwrap();
        for expected in frames.iter() {
            let actual = tokio::time::timeout(Duration::from_secs(5), socket.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            match (expected, actual) {
                (AxumWsMessage::Text(expected), ClientWsMessage::Text(actual)) => {
                    assert_eq!(actual.as_bytes(), expected.as_bytes());
                }
                (AxumWsMessage::Binary(expected), ClientWsMessage::Binary(actual)) => {
                    assert_eq!(&actual, expected);
                }
                (_, actual) => panic!("frame representation changed: {actual:?}"),
            }
        }
    }
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    for event in events.iter() {
        assert!(event.success);
        assert_eq!(event.input_tokens, Some(11));
        assert_eq!(event.cached_input_tokens, Some(7));
        assert_eq!(event.output_tokens, Some(5));
        assert!(event.ttft_ms.is_some());
    }
}

#[tokio::test]
async fn websocket_tool_owners_survive_reconnect_only_after_completed_responses() {
    for (custom, completed, reconnect) in [
        (false, true, false),
        (true, true, true),
        (false, false, true),
        (true, false, false),
    ] {
        let mut call = json!({
            "type": if custom { "custom_tool_call" } else { "function_call" },
            "call_id": "call_native", "name": "lookup"
        });
        call[if custom { "input" } else { "arguments" }] = json!("{}");
        let terminal_type = if completed {
            "response.completed"
        } else {
            "response.incomplete"
        };
        let (upstream, state) = spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type":"response.output_text.delta", "delta":"synthetic text"}),
            json!({"type":"response.output_item.done", "item":call}),
            json!({"type":terminal_type, "response":{
                "id":"tool-response", "status":if completed { "completed" } else { "incomplete" },
                "output":[], "incomplete_details":{"reason":"max_output_tokens"},
                "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
            }}),
        ]))).await;
        let (gateway, _, _, _) = spawn_mixed_gateway(
            vec![source("tool-source", &upstream, "synthetic-key", 100)],
            Vec::new(),
            vec![mixed_key(None, None)],
            Arc::new(TokenAuthority::new(1).unwrap()),
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
                    "type":"response.create", "model":MODEL, "input":"synthetic question"
                })
                .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.output_text.delta"
        );
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            "response.output_item.done"
        );
        assert_eq!(
            receive_websocket_json(&mut socket).await["type"],
            terminal_type
        );
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
        let input = json!([{
            "type":if custom { "custom_tool_call_output" } else { "function_call_output" },
            "call_id":"call_native", "output":"synthetic result"
        }]);
        socket
            .send(ClientWsMessage::Text(
                json!({
                    "type":"response.create", "model":MODEL, "input":input
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let next = receive_websocket_json(&mut socket).await;
        if completed {
            assert_eq!(next["type"], "response.output_text.delta");
            receive_websocket_completion(&mut socket).await;
        } else {
            assert_eq!(next["error"]["code"], "response_continuation_unavailable");
        }
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), if completed { 2 } else { 1 });
        if completed {
            assert_eq!(requests[1]["input"], input);
            assert!(requests[1].get("previous_response_id").is_none());
        }
    }
}

#[tokio::test]
async fn account_websocket_preserves_codex_headers_and_reports_usage() {
    let (upstream, state) = spawn_websocket_upstream().await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
        GatewayRuntimeOptions {
            diagnostics_enabled: || true,
            default_service_tier: DefaultServiceTier::Fast,
            ..reference_metadata_options()
        },
    )
    .await;
    let catalog = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
            gateway.base_url
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(catalog.status(), StatusCode::OK);

    let rejected = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth("wrong-local-key")
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    assert!(state.headers.lock().unwrap().is_empty());

    let upgraded = reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("user-agent", "codex_test/1.0.0")
        .header("version", "1.0.0")
        .header("originator", "codex_test")
        .header("openai-beta", "existing_feature=1")
        .header("x-openai-internal-codex-responses-lite", "true")
        .upgrade()
        .send()
        .await
        .unwrap();
    assert_eq!(upgraded.status(), StatusCode::SWITCHING_PROTOCOLS);
    let mut socket = upgraded.into_websocket().await.unwrap();

    for index in 0..2 {
        let mut request = json!({
            "type": "response.create",
            "model": MODEL,
            "input": format!("hello {index}"),
            "parallel_tool_calls": false
        });
        if index == 1 {
            request["service_tier"] = Value::String("flex".to_string());
            request["reasoning"] = json!({
                "effort": "high",
                "summary": "detailed",
                "context": "previous_turn"
            });
        }
        socket
            .send(ClientWsMessage::Text(request.to_string()))
            .await
            .unwrap();
        let completed = receive_websocket_completion(&mut socket).await;
        assert_eq!(completed["response"]["usage"]["input_tokens"], 11);
    }

    let headers = state.headers.lock().unwrap();
    assert_eq!(headers.len(), 2);
    for headers in headers.iter() {
        assert_eq!(
            header(headers, AUTHORIZATION.as_str()).as_deref(),
            Some("Bearer account-access")
        );
        assert_eq!(
            header(headers, "chatgpt-account-id").as_deref(),
            Some("provider-account")
        );
        assert_eq!(header(headers, "originator").as_deref(), Some("codex_test"));
        assert_eq!(
            header(headers, "user-agent").as_deref(),
            Some("codex_test/1.0.0")
        );
        assert_eq!(header(headers, "version").as_deref(), Some("1.0.0"));
        assert_eq!(
            header(headers, "x-openai-internal-codex-responses-lite").as_deref(),
            Some("true")
        );
        let beta = headers
            .get_all("openai-beta")
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect::<Vec<_>>()
            .join(",");
        assert!(beta.contains("existing_feature=1"));
        assert!(beta.contains("responses_websockets=2026-02-06"));
    }
    drop(headers);

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["type"], "response.create");
    assert_eq!(requests[0]["store"], false);
    assert_eq!(requests[0]["stream"], true);
    assert_eq!(requests[0]["parallel_tool_calls"], false);
    // A managed Codex client can select its native speed. The pool default
    // applies only to the request that omits the field.
    assert_eq!(requests[0]["service_tier"], "priority");
    assert_eq!(requests[1]["service_tier"], "flex");
    assert_eq!(requests[1]["reasoning"]["effort"], "high");
    assert_eq!(requests[1]["reasoning"]["summary"], "detailed");
    assert_eq!(requests[1]["reasoning"]["context"], "all_turns");
    assert!(requests[0]["input"].is_array());
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
    assert!(events.iter().all(|event| event.input_tokens == Some(11)));
    assert!(
        events
            .iter()
            .all(|event| event.cached_input_tokens == Some(7))
    );
    assert!(events.iter().all(|event| event.output_tokens == Some(5)));
    assert!(events.iter().all(|event| event.reasoning_tokens == Some(2)));
    assert!(events.iter().all(|event| event.total_tokens == Some(16)));
    assert!(events.iter().all(|event| event.ttft_ms.is_some()));
    for event in events.iter() {
        let transport = serde_json::to_value(&event.routing.as_ref().unwrap().transport).unwrap();
        assert_eq!(transport["upstream"], "websocket");
        assert_eq!(transport["mode"], "native");
        assert_eq!(transport["responseFormat"], "websocket_frames");
        assert!(transport["fallbackReason"].is_null());
        assert!(transport["handshakeMs"].is_number());
    }
}

#[tokio::test]
async fn websocket_model_switch_resets_incompatible_response_owner_before_reconnect() {
    let (old_model_upstream, old_state) = spawn_replayable_websocket_upstream().await;
    let (new_model_upstream, new_state) = spawn_websocket_upstream().await;
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
                "model": "old-model",
                "input": "start"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let first = receive_websocket_completion(&mut socket).await;
    let first_response_id = first["response"]["id"].as_str().unwrap().to_string();

    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": "new-model",
                "input": [
                    {"type": "message", "role": "user", "content": "start"},
                    {"type": "message", "role": "assistant", "content": "old-model response"},
                    {"type": "message", "role": "user", "content": "continue after switching models"}
                ],
                "previous_response_id": first_response_id
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut socket).await;
    let _ = receive_websocket_completion(&mut socket).await;

    assert_eq!(old_state.requests.lock().unwrap().len(), 1);
    let requests = new_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].get("previous_response_id").is_none());
    assert_eq!(requests[0]["model"], "new-model");
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn websocket_model_switch_resets_incompatible_response_owner_on_new_connection() {
    let (old_model_upstream, old_state) = spawn_replayable_websocket_upstream().await;
    let (new_model_upstream, new_state) = spawn_websocket_upstream().await;
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
            json!({
                "type": "response.create",
                "model": "old-model",
                "input": "start"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let first = receive_websocket_completion(&mut first_socket).await;
    let first_response_id = first["response"]["id"].as_str().unwrap().to_string();
    drop(first_socket);

    let upgraded = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap();
    let mut second_socket = upgraded.into_websocket().await.unwrap();
    second_socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": "new-model",
                "input": [
                    {"type": "message", "role": "user", "content": "start"},
                    {"type": "message", "role": "assistant", "content": "old-model response"},
                    {"type": "message", "role": "user", "content": "continue after switching models"}
                ],
                "previous_response_id": first_response_id
            })
            .to_string(),
        ))
        .await
        .unwrap();
    let _ = receive_websocket_json(&mut second_socket).await;
    let _ = receive_websocket_completion(&mut second_socket).await;

    assert_eq!(old_state.requests.lock().unwrap().len(), 1);
    let requests = new_state.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(requests[0].get("previous_response_id").is_none());
    assert_eq!(requests[0]["model"], "new-model");
    drop(requests);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.success));
}

#[tokio::test]
async fn account_websocket_retries_foreign_message_item_id_after_native_rejection() {
    let (upstream, state) = spawn_websocket_upstream_with_behavior(WebSocketBehavior::Sequence(
        Arc::new(Mutex::new(VecDeque::from(vec![
            vec![json!({
                "type": "error",
                "status": 400,
                "error": {
                    "message": "Invalid 'input[0].id': 'item_foreign_user_01'. Expected an ID that begins with 'msg'.",
                    "type": "invalid_request_error"
                }
            })],
            vec![
                json!({"type": "response.output_text.delta", "delta": "repaired"}),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "ws-message-id-repaired",
                        "status": "completed",
                        "output": [],
                        "usage": {"input_tokens": 3, "output_tokens": 1, "total_tokens": 4}
                    }
                }),
            ],
        ]))),
    ))
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
                "model": MODEL,
                "input": [
                    {
                        "type": "message",
                        "id": "item_foreign_user_01",
                        "role": "user",
                        "content": [{"type": "input_text", "text": "Inspect the workspace"}]
                    },
                    {
                        "type": "message",
                        "id": "msg_native_01",
                        "role": "developer",
                        "content": [{"type": "input_text", "text": "Keep changes scoped"}]
                    },
                    {
                        "type": "reasoning",
                        "id": "item_reasoning_01",
                        "summary": []
                    }
                ]
            })
            .to_string(),
        ))
        .await
        .unwrap();

    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "ws-message-id-repaired"
    );

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["input"][0]["id"], "item_foreign_user_01");
    assert!(requests[1]["input"][0].get("id").is_none());
    assert_eq!(requests[1]["input"][1]["id"], "msg_native_01");
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(!events[0].success);
    assert_eq!(
        events[0].routing.as_ref().unwrap().attempt_outcome,
        Some(zenith_relay_core::AttemptOutcome::RetryRepair)
    );
    assert!(events[1].success);
    assert_eq!(events[0].request_id, events[1].request_id);
}

#[tokio::test]
async fn account_websocket_repairs_legacy_call_ids_on_a_later_request() {
    let (upstream, state) = spawn_websocket_upstream_with_behavior(WebSocketBehavior::Sequence(
        Arc::new(Mutex::new(VecDeque::from(vec![
            vec![
                json!({"type": "response.output_text.delta", "delta": "first"}),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "ws-first-call-id",
                        "status": "completed",
                        "output": [],
                        "usage": {"input_tokens": 2, "output_tokens": 1, "total_tokens": 3}
                    }
                }),
            ],
            vec![json!({
                "type": "error",
                "status": 400,
                "error": {"message": "Missing required field: call_id"}
            })],
            vec![
                json!({"type": "response.output_text.delta", "delta": "repaired"}),
                json!({
                    "type": "response.completed",
                    "response": {
                        "id": "ws-repaired-call-id",
                        "status": "completed",
                        "output": [],
                        "usage": {"input_tokens": 4, "output_tokens": 1, "total_tokens": 5}
                    }
                }),
            ],
        ]))),
    ))
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
                "model": MODEL,
                "input": "start"
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "ws-first-call-id"
    );

    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "previous_response_id": "ws-first-call-id",
                "input": [
                    {"type": "function_call", "name": "lookup", "arguments": "{}"},
                    {"type": "function_call_output", "output": "result"}
                ]
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "ws-repaired-call-id"
    );

    tokio::time::sleep(Duration::from_millis(20)).await;

    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    assert!(requests[1]["input"][0].get("call_id").is_none());
    assert_eq!(
        requests[2]["input"][0]["call_id"],
        requests[2]["input"][1]["call_id"]
    );
    drop(requests);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events.iter().filter(|event| event.success).count(), 2);
    assert!(!events[1].success);
}

#[tokio::test]
async fn account_websocket_keeps_connection_after_max_output_incomplete() {
    let responses = VecDeque::from(vec![
        vec![
            json!({"type": "response.output_text.delta", "delta": "partial"}),
            json!({
                "type": "response.incomplete",
                "response": {
                    "id": "ws-incomplete",
                    "status": "incomplete",
                    "output": [],
                    "incomplete_details": {"reason": "max_output_tokens"},
                    "usage": {"input_tokens": 3, "output_tokens": 4, "total_tokens": 7}
                }
            }),
        ],
        vec![
            json!({"type": "response.output_text.delta", "delta": "continued"}),
            json!({
                "type": "response.completed",
                "response": {
                    "id": "ws-completed",
                    "status": "completed",
                    "output": [],
                    "usage": {"input_tokens": 5, "output_tokens": 6, "total_tokens": 11}
                }
            }),
        ],
    ]);
    let (upstream, state) = spawn_websocket_upstream_with_behavior(WebSocketBehavior::Sequence(
        Arc::new(Mutex::new(responses)),
    ))
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
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
    assert_eq!(
        receive_websocket_json(&mut socket).await["delta"],
        "partial"
    );
    let incomplete = receive_websocket_json(&mut socket).await;
    assert_eq!(incomplete["type"], "response.incomplete");
    assert_eq!(
        incomplete["response"]["incomplete_details"]["reason"],
        "max_output_tokens"
    );

    let foreign = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":MODEL,"input":"continue elsewhere",
            "previous_response_id":incomplete["response"]["id"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), StatusCode::CONFLICT);
    assert_eq!(state.requests.lock().unwrap().len(), 1);

    socket
        .send(ClientWsMessage::Text(
            json!({
                "type": "response.create",
                "model": MODEL,
                "input": "continue",
                "previous_response_id": incomplete["response"]["id"]
            })
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(
        receive_websocket_json(&mut socket).await["delta"],
        "continued"
    );
    assert_eq!(
        receive_websocket_completion(&mut socket).await["response"]["id"],
        "ws-completed"
    );
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert_eq!(state.requests.lock().unwrap().len(), 2);
    assert_eq!(state.headers.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    let first = serde_json::to_value(&events[0].routing.as_ref().unwrap().transport).unwrap();
    let reused = serde_json::to_value(&events[1].routing.as_ref().unwrap().transport).unwrap();
    assert!(first["handshakeMs"].is_number());
    assert!(reused["handshakeMs"].is_null());
    assert_eq!(reused["upstream"], "websocket");
    assert_eq!(reused["responseFormat"], "websocket_frames");
    assert!(!events[0].success);
    assert_eq!(events[0].http_status, StatusCode::OK.as_u16());
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("response_incomplete")
    );
    assert!(events[1].success);
}
