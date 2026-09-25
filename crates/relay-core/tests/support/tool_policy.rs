use super::*;
use zenith_relay_core::{ToolPolicy, ToolPolicyMode, ToolPolicyOutcome};

async fn capture(
    State(bodies): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> Response<Body> {
    bodies.lock().unwrap().push(body.clone());
    let response = json!({"id":"resp_synthetic","object":"response","status":"completed",
        "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic"}]}],
        "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}});
    if body["stream"] == true {
        Response::builder()
            .header(CONTENT_TYPE, "text/event-stream")
            .body(Body::from(format!(
                "data: {}\n\n",
                json!({"type":"response.completed","response":response})
            )))
            .unwrap()
    } else {
        Json(response).into_response()
    }
}

fn policy() -> ToolPolicy {
    ToolPolicy::default()
}

fn request() -> Value {
    json!({"model":"gpt-test","input":"synthetic request","tools":(0..73).map(|i|json!({
        "type":"function","name":format!("tool_{i}"),"parameters":{"type":"object"}
    })).collect::<Vec<_>>()})
}

fn automatic_policy() -> ToolPolicy {
    ToolPolicy {
        mode: ToolPolicyMode::Automatic,
    }
}

fn has_deferred_tool_search(body: &Value) -> bool {
    body["tools"].as_array().is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool["type"] == "tool_search" || tool["defer_loading"].as_bool() == Some(true)
        })
    })
}

async fn reject_deferred_tool_search(
    State(bodies): State<Arc<Mutex<Vec<Value>>>>,
    Json(body): Json<Value>,
) -> Response<Body> {
    bodies.lock().unwrap().push(body.clone());
    if has_deferred_tool_search(&body) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {
                    "type": "invalid_request_error",
                    "code": "unsupported_tool_search",
                    "message": "tool_search and defer_loading are not supported"
                }
            })),
        )
            .into_response();
    }
    Json(json!({
        "id":"resp_compatibility_retry",
        "object":"response",
        "status":"completed",
        "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic"}]}],
        "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
    }))
    .into_response()
}

async fn stream_parser_rejects_deferred_then_accepts_original(
    State(bodies): State<Arc<Mutex<Vec<Value>>>>,
    body: Bytes,
) -> Response<Body> {
    let body: Value = serde_json::from_slice(&body).unwrap();
    let deferred = has_deferred_tool_search(&body);
    bodies.lock().unwrap().push(body);
    let payload = if deferred {
        "data: {\n\n".to_string()
    } else {
        format!(
            "data: {}\n\n",
            json!({
                "type":"response.completed",
                "response": {
                    "id":"resp_stream_retry",
                    "object":"response",
                    "status":"completed",
                    "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"synthetic"}]}],
                    "usage":{"input_tokens":1,"output_tokens":1,"total_tokens":2}
                }
            })
        )
    };
    Response::builder()
        .header(CONTENT_TYPE, "text/event-stream")
        .body(Body::from(payload))
        .unwrap()
}

#[tokio::test]
async fn automatic_native_responses_uses_provider_deferred_tool_search_and_records_usage() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(
        Router::new()
            .route("/v1/responses", post(capture))
            .with_state(bodies.clone()),
    )
    .await;
    let events = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
    let captured_events = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::new(
            ProviderSource {
                id: "synthetic".into(),
                name: "Synthetic".into(),
                base_url: format!("{}/v1", upstream.base_url),
                api_key: SOURCE_KEY.into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-test".into()],
            },
            LocalGatewayKey {
                id: "synthetic".into(),
                secret: LOCAL_KEY.into(),
            },
            Arc::new(move |event| captured_events.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    runtime.set_tool_policy(automatic_policy()).unwrap();
    let gateway = spawn(gateway::router(runtime)).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let captured = bodies.lock().unwrap();
    assert_eq!(captured.len(), 1);
    let tools = captured[0]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 74);
    assert!(tools[..73].iter().all(|tool| tool["defer_loading"] == true));
    assert_eq!(tools[73], json!({"type":"tool_search"}));
    drop(captured);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    let diagnostics = &events[0].tool_use;
    assert!(events[0].success);
    assert_eq!(diagnostics.client_tool_count, 73);
    assert_eq!(diagnostics.forwarded_tool_count, 73);
    assert_eq!(diagnostics.filtered_tool_count, 0);
    assert_eq!(
        diagnostics.policy_outcome,
        Some(ToolPolicyOutcome::Deferred)
    );
    assert!(diagnostics.deferred_tool_search);
    assert!(!diagnostics.policy_fallback);
    assert!(diagnostics.client_schema_bytes < diagnostics.forwarded_schema_bytes);
}

#[tokio::test]
async fn deferred_tool_search_rejection_retries_without_deferred_fields_and_keeps_full_catalog() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(
        Router::new()
            .route("/v1/responses", post(reject_deferred_tool_search))
            .with_state(bodies.clone()),
    )
    .await;
    let events = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
    let captured_events = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::new(
            ProviderSource {
                id: "synthetic".into(),
                name: "Synthetic".into(),
                base_url: format!("{}/v1", upstream.base_url),
                api_key: SOURCE_KEY.into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-test".into()],
            },
            LocalGatewayKey {
                id: "synthetic".into(),
                secret: LOCAL_KEY.into(),
            },
            Arc::new(move |event| captured_events.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    runtime.set_tool_policy(automatic_policy()).unwrap();
    let gateway = spawn(gateway::router(runtime)).await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&request())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2);
    let first_tools = bodies[0]["tools"].as_array().unwrap();
    assert!(has_deferred_tool_search(&bodies[0]));
    assert_eq!(first_tools.len(), 74);
    let second_tools = bodies[1]["tools"].as_array().unwrap();
    assert!(!has_deferred_tool_search(&bodies[1]));
    assert_eq!(second_tools.len(), 73);
    assert_eq!(second_tools[72]["name"], "tool_72");
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(!events[0].success);
    assert!(events[0].tool_use.deferred_tool_search);
    assert!(events[0].tool_use.policy_fallback);
    assert_eq!(events[0].tool_use.filtered_tool_count, 0);
    assert!(events[1].success);
    assert!(!events[1].tool_use.deferred_tool_search);
    assert!(events[1].tool_use.policy_fallback);
    assert_eq!(events[1].tool_use.forwarded_tool_count, 73);
    assert_eq!(events[1].tool_use.filtered_tool_count, 0);
}

#[tokio::test]
async fn deferred_tool_search_parser_failure_does_not_replay_unknown_execution() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(
        Router::new()
            .route(
                "/v1/responses",
                post(stream_parser_rejects_deferred_then_accepts_original),
            )
            .with_state(bodies.clone()),
    )
    .await;
    let events = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
    let captured_events = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::new(
            ProviderSource {
                id: "synthetic".into(),
                name: "Synthetic".into(),
                base_url: format!("{}/v1", upstream.base_url),
                api_key: SOURCE_KEY.into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-test".into()],
            },
            LocalGatewayKey {
                id: "synthetic".into(),
                secret: LOCAL_KEY.into(),
            },
            Arc::new(move |event| captured_events.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    runtime.set_tool_policy(automatic_policy()).unwrap();
    let gateway = spawn(gateway::router(runtime)).await;
    let mut body = request();
    body["stream"] = json!(true);

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(!response.text().await.unwrap().contains("resp_stream_retry"));

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert!(has_deferred_tool_search(&bodies[0]));
    drop(bodies);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(!events[0].success);
    assert_eq!(events[0].error_category.as_deref(), Some("stream_invalid"));
    assert!(events[0].tool_use.deferred_tool_search);
    assert!(!events[0].tool_use.policy_fallback);
}

#[tokio::test]
async fn tool_policy_forwards_the_complete_catalog_across_http_sse_and_websocket() {
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let upstream = spawn(
        Router::new()
            .route("/v1/responses", post(capture))
            .with_state(bodies.clone()),
    )
    .await;
    let events = Arc::new(Mutex::new(Vec::<UsageEvent>::new()));
    let captured_events = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::new(
            ProviderSource {
                id: "synthetic".into(),
                name: "Synthetic".into(),
                base_url: format!("{}/v1", upstream.base_url),
                api_key: SOURCE_KEY.into(),
                wire_api: WireApi::Responses,
                models: vec!["gpt-test".into()],
            },
            LocalGatewayKey {
                id: "synthetic".into(),
                secret: LOCAL_KEY.into(),
            },
            Arc::new(move |event| captured_events.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    runtime.set_tool_policy(policy()).unwrap();
    let gateway = spawn(gateway::router(runtime.clone())).await;
    let client = reqwest::Client::new();
    for stream in [false, true] {
        let mut body = request();
        body["stream"] = json!(stream);
        let response = client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.text().await.unwrap().contains("resp_synthetic"));
    }
    let mut socket = client
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    let mut body = request();
    body["type"] = json!("response.create");
    socket
        .send(ClientWsMessage::Text(body.to_string()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = socket.next().await {
            if let ClientWsMessage::Text(text) = message.unwrap() {
                let event: Value = serde_json::from_str(&text).unwrap();
                assert_ne!(event["type"], "error", "{event}");
                if event["type"] == "response.completed" {
                    return;
                }
            }
        }
        panic!("socket ended before terminal response");
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while events.lock().unwrap().len() < 3 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    {
        let captured = bodies.lock().unwrap();
        assert_eq!(captured.len(), 3);
        for body in captured.iter() {
            assert_eq!(body["tools"].as_array().unwrap().len(), 73);
            assert_eq!(body["tools"][72]["name"], "tool_72");
            assert_eq!(body["input"], "synthetic request");
        }
        let events = events.lock().unwrap();
        for event in events.iter() {
            let diagnostics = &event.tool_use;
            assert_eq!(
                (
                    diagnostics.client_tool_count,
                    diagnostics.forwarded_tool_count
                ),
                (73, 73)
            );
            assert_eq!(diagnostics.filtered_tool_count, 0);
            assert_eq!(
                diagnostics.policy_outcome,
                Some(ToolPolicyOutcome::PassThrough)
            );
            assert_eq!(
                diagnostics.client_schema_bytes,
                diagnostics.forwarded_schema_bytes
            );
        }
    }

    // A hot update enables provider-native optimization without restarting the
    // listener. The complete catalog remains available on the wire.
    runtime.set_tool_policy(automatic_policy()).unwrap();
    assert!(client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&request())
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    {
        let captured = bodies.lock().unwrap();
        let body = captured.last().unwrap();
        assert!(has_deferred_tool_search(body));
        assert_eq!(body["tools"].as_array().unwrap().len(), 74);
    }
    runtime.set_tool_policy(ToolPolicy::default()).unwrap();
    assert!(client
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&request())
        .send()
        .await
        .unwrap()
        .status()
        .is_success());
    assert_eq!(
        bodies.lock().unwrap().last().unwrap()["tools"]
            .as_array()
            .unwrap()
            .len(),
        73
    );

    // A specific client choice no longer conflicts with a Relay-side selector:
    // Relay does not remove the selected tool or any other declaration.
    runtime.set_tool_policy(automatic_policy()).unwrap();
    let mut conflict = request();
    conflict["tool_choice"] = json!({"type":"function","name":"tool_72"});
    assert_eq!(
        client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&conflict)
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let captured = bodies.lock().unwrap();
    assert_eq!(captured.len(), 6);
    assert!(!has_deferred_tool_search(captured.last().unwrap()));
    assert_eq!(
        captured.last().unwrap()["tools"].as_array().unwrap().len(),
        73
    );
}
