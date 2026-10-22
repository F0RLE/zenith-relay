use super::*;

fn memory_runtime(
    upstream: &TestServer,
    events: Arc<Mutex<Vec<UsageEvent>>>,
) -> Arc<GatewayRuntime> {
    Arc::new(
        GatewayRuntime::from_pool(
            vec![source(
                "memory-source",
                upstream,
                "synthetic-upstream-key",
                10,
            )],
            vec![zenith_relay_core::RuntimeLocalKey::unrestricted(
                LocalGatewayKey {
                    id: "memory-key".into(),
                    secret: LOCAL_KEY.into(),
                },
            )],
            reference_metadata_options(),
            Arc::new(move |event| events.lock().unwrap().push(event)),
        )
        .unwrap(),
    )
}

fn memory_metadata(extraction: bool) -> Value {
    json!({
        "request_kind": if extraction { "memory" } else { "turn" },
        "thread_source": "memory_consolidation",
        "turn_trigger": "memory_consolidation"
    })
}

#[tokio::test]
async fn memory_policy_http_blocks_before_dispatch_and_preserves_chat_and_other_stage() {
    let response = json!({
        "id":"synthetic-memory", "object":"response", "status":"completed", "model":MODEL,
        "output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":"ok"}]}],
        "usage":{"input_tokens":4,"output_tokens":1,"total_tokens":5}
    });
    let (upstream, state) = spawn_upstream(vec![Reply::Json(StatusCode::OK, response); 4]).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let runtime = memory_runtime(&upstream, events.clone());
    runtime.set_codex_background_tasks_enabled(false);
    let gateway = spawn(gateway::router(runtime.clone())).await;
    let client = reqwest::Client::new();
    for extraction in [true, false] {
        runtime.set_codex_memory_extraction_enabled(!extraction);
        runtime.set_codex_memory_consolidation_enabled(extraction);
        let stage = if extraction {
            "extraction"
        } else {
            "consolidation"
        };
        for stream in [false, true] {
            let rejected = client
                .post(format!("{}/v1/responses", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .header("originator", "chatgpt desktop")
                .header(
                    "x-codex-turn-metadata",
                    memory_metadata(extraction).to_string(),
                )
                .json(&json!({"model":MODEL,"input":"synthetic memory task","stream":stream}))
                .send()
                .await
                .unwrap();
            assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
            let body: Value = rejected.json().await.unwrap();
            assert_eq!(
                body["error"]["code"],
                format!("codex_background_blocked_memory_{stage}")
            );
            assert!(body.get("output").is_none());
        }
        let allowed = client.post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model":MODEL,"input":"synthetic other stage","client_metadata":memory_metadata(!extraction)}))
            .send().await.unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
    }
    runtime.set_codex_memory_extraction_enabled(false);
    runtime.set_codex_memory_consolidation_enabled(false);
    for kind in ["turn", "compaction"] {
        let allowed = client.post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .header("originator", "chatgpt desktop")
            .json(&json!({"model":MODEL,"input":"discuss memory consolidation","client_metadata":{"request_kind":kind,"thread_source":"user"}}))
            .send().await.unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
    }
    assert_eq!(state.requests.lock().unwrap().len(), 4);
    let events = events.lock().unwrap();
    let blocked = events
        .iter()
        .filter(|event| event.http_status == 403)
        .collect::<Vec<_>>();
    assert_eq!(blocked.len(), 4);
    for event in blocked {
        assert!(!event.success);
        assert_eq!(event.total_tokens, Some(0));
        assert!(event.account_id.is_none());
        assert_eq!(
            event.error_origin(),
            Some(zenith_relay_core::ErrorOrigin::Relay)
        );
    }
    assert!(events
        .iter()
        .any(|event| event.error_category.as_deref() == Some("codex_memory_extraction")));
    assert!(events
        .iter()
        .any(|event| event.error_category.as_deref() == Some("codex_memory_consolidation")));
}

#[tokio::test]
async fn memory_policy_websocket_rejects_both_stages_without_upstream_or_fake_success() {
    let (upstream, state) = spawn_upstream(Vec::new()).await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let runtime = memory_runtime(&upstream, events.clone());
    runtime.set_codex_memory_extraction_enabled(false);
    runtime.set_codex_memory_consolidation_enabled(false);
    let gateway = spawn(gateway::router(runtime)).await;
    for extraction in [true, false] {
        let upgraded = reqwest::Client::new()
            .get(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .upgrade()
            .send()
            .await
            .unwrap();
        let mut socket = upgraded.into_websocket().await.unwrap();
        socket.send(ClientWsMessage::Text(json!({
            "type":"response.create", "stream_id":"synthetic-stream", "model":MODEL,
            "input":"synthetic memory", "client_metadata":{"x-codex-turn-metadata":memory_metadata(extraction).to_string()}
        }).to_string())).await.unwrap();
        let error = receive_websocket_json(&mut socket).await;
        assert_eq!(error["type"], "error");
        assert_eq!(error["status"], 403);
        assert_eq!(error["stream_id"], "synthetic-stream");
        let stage = if extraction {
            "extraction"
        } else {
            "consolidation"
        };
        assert_eq!(
            error["error"]["code"],
            format!("codex_background_blocked_memory_{stage}")
        );
        assert_eq!(error["error"]["zenith_relay"]["origin"], "relay");
    }
    assert!(state.requests.lock().unwrap().is_empty());
    assert_eq!(events.lock().unwrap().len(), 2);
}
