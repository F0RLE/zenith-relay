use super::*;

async fn gateway_with_backup(
    upstream: &TestServer,
    backup: &TestServer,
) -> (TestServer, Arc<Mutex<Vec<UsageEvent>>>) {
    let authority = Arc::new(TokenAuthority::new(2).unwrap());
    register_ready(&authority, "continuation-owner", "synthetic-owner-access").await;
    register_ready(&authority, "continuation-backup", "synthetic-backup-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("continuation-owner", "provider-owner", upstream, 200),
            account("continuation-backup", "provider-backup", backup, 100),
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
        .set_route_recovery_enabled(true);
    rotation_policy::set_order(&gateway, &["continuation-owner", "continuation-backup"]);
    (gateway, events)
}

async fn connect(gateway: &TestServer) -> reqwest_websocket::WebSocket {
    reqwest::Client::new()
        .get(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap()
}

#[tokio::test]
async fn encrypted_continuation_waits_on_its_original_owner_for_429_and_503() {
    for (status, code) in [(429, "rate_limit_exceeded"), (503, "server_overloaded")] {
        let agent = json!({"type":"agent_message","author":"/root","recipient":"/root/worker",
            "encrypted_content":"synthetic-native-agent-ciphertext"});
        let call = json!({"type":"function_call","call_id":"call_synthetic_task",
            "name":"synthetic_tool","arguments":"{}"});
        let first = json!({"type":"response.completed","response":{
            "id":"resp_synthetic_owner","status":"completed","model":MODEL,
            "output":[agent,call],"usage":{"input_tokens":5,"output_tokens":2,"total_tokens":7}
        }});
        let refusal = json!({"type":"error","status":status,"body":{"error":{
            "code":code,"message":"Synthetic temporary rejection","retry_after":0.01
        }}});
        let completed = json!({"type":"response.completed","response":{
            "id":"resp_synthetic_resumed","status":"completed","model":MODEL,
            "output":[{"type":"message","role":"assistant","content":[{
                "type":"output_text","text":"Synthetic task resumed"
            }]}],"usage":{"input_tokens":6,"output_tokens":2,"total_tokens":8}
        }});
        let (upstream, state) = spawn_websocket_upstream_with_behavior(
            WebSocketBehavior::Sequence(Arc::new(Mutex::new(VecDeque::from([
                vec![first],
                vec![refusal],
                vec![completed],
            ])))),
        )
        .await;
        let (backup, backup_state) = spawn_websocket_upstream().await;
        let (gateway, _) = gateway_with_backup(&upstream, &backup).await;
        let mut socket = connect(&gateway).await;
        socket
            .send(ClientWsMessage::Text(
                json!({"type":"response.create","model":MODEL,
                    "input":"Synthetic task","tools":[{"type":"function","name":"synthetic_tool",
                    "parameters":{"type":"object","properties":{},"additionalProperties":false}}]
                })
                .to_string(),
            ))
            .await
            .unwrap();
        let delivered = receive_websocket_completion(&mut socket).await;
        assert_eq!(delivered["type"], "response.completed");
        let input = json!([agent, {"type":"function_call_output","call_id":"call_synthetic_task",
            "output":"synthetic tool result"}]);
        socket
            .send(ClientWsMessage::Text(
                json!({"type":"response.create","model":MODEL,
                    "previous_response_id":"resp_synthetic_owner","input":input
                })
                .to_string(),
            ))
            .await
            .unwrap();
        assert_eq!(
            receive_websocket_completion(&mut socket).await["type"],
            "response.completed"
        );
        let requests = state.requests.lock().unwrap();
        assert_eq!(requests.len(), 3, "status {status}");
        assert_eq!(requests[1]["input"], input);
        assert_eq!(requests[2]["input"], input);
        assert_eq!(requests[1]["previous_response_id"], "resp_synthetic_owner");
        assert_eq!(requests[2]["previous_response_id"], "resp_synthetic_owner");
        assert!(backup_state.requests.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn codex_metadata_then_close_produces_one_failure_without_replay_or_first_token() {
    let frames = vec![
        AxumWsMessage::Text(
            json!({"type":"codex.rate_limits","rate_limits":{}})
                .to_string()
                .into(),
        ),
        AxumWsMessage::Text(
            json!({"type":"codex.response.metadata","response_id":"resp_synthetic"})
                .to_string()
                .into(),
        ),
        AxumWsMessage::Close(None),
    ];
    let (upstream, state) =
        spawn_websocket_upstream_with_behavior(WebSocketBehavior::Frames(Arc::new(frames))).await;
    let (backup, backup_state) = spawn_websocket_upstream().await;
    let (gateway, events) = gateway_with_backup(&upstream, &backup).await;
    let mut socket = connect(&gateway).await;
    socket
        .send(ClientWsMessage::Text(
            json!({"type":"response.create","model":MODEL,
        "input":"Synthetic metadata-only request"})
            .to_string(),
        ))
        .await
        .unwrap();
    assert_eq!(receive_websocket_json(&mut socket).await["type"], "error");
    tokio::time::timeout(Duration::from_secs(2), async {
        while usage_observations::physical_attempts(&events.lock().unwrap()).is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    assert!(backup_state.requests.lock().unwrap().is_empty());
    let events = events.lock().unwrap();
    let attempts = usage_observations::physical_attempts(&events);
    assert_eq!(attempts.len(), 1);
    assert!(!attempts[0].success);
    assert_eq!(attempts[0].ttft_ms, None);
    assert_eq!(attempts[0].input_tokens, None);
    assert_eq!(attempts[0].output_tokens, None);
    assert_eq!(
        attempts[0].error_category.as_deref(),
        Some("upstream_websocket_closed")
    );
}
