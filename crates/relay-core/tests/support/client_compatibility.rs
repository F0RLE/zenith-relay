use super::*;
use std::io::Write;

#[tokio::test]
async fn websocket_credential_change_reconnects_only_with_portable_history() {
    for portable in [true, false] {
        let output = if portable {
            json!([{"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":"synthetic answer"}]}])
        } else {
            json!([{"type":"compaction", "encrypted_content":"synthetic-opaque-state"}])
        };
        let (upstream, observed) = spawn_websocket_upstream_with_behavior(WebSocketBehavior::Events(Arc::new(vec![
            json!({"type":"response.completed", "response":{"id":"synthetic-response", "output":output}}),
        ]))).await;
        let authority = ready_authority("compat-account", "synthetic-old").await;
        let (gateway, _, _, _) = spawn_mixed_gateway(
            vec![],
            vec![account(
                "compat-account",
                "synthetic-provider",
                &upstream,
                10,
            )],
            vec![mixed_key(None, None)],
            authority.clone(),
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
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
                json!({"type":"response.create", "model":MODEL, "input":"synthetic input"})
                    .to_string(),
            ))
            .await
            .unwrap();
        receive_websocket_completion(&mut socket).await;
        authority
            .register(
                "compat-account",
                TokenSet::access_only(
                    "synthetic-new",
                    Some(current_time_ms() + 600_000),
                    current_time_ms(),
                )
                .unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        socket.send(ClientWsMessage::Text(json!({"type":"response.create", "model":MODEL, "previous_response_id":"synthetic-response", "input":"synthetic follow-up"}).to_string())).await.unwrap();
        if portable {
            receive_websocket_completion(&mut socket).await;
            let headers = observed.headers.lock().unwrap();
            assert_eq!(headers.len(), 2);
            assert_eq!(
                header(&headers[1], "authorization").as_deref(),
                Some("Bearer synthetic-new")
            );
            let requests = observed.requests.lock().unwrap();
            assert_eq!(requests.len(), 2);
            assert!(requests[1].get("previous_response_id").is_none());
            assert!(requests[1]["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["role"] == "assistant"));
        } else {
            let failure = receive_websocket_json(&mut socket).await;
            assert_eq!(
                failure["error"]["code"],
                "response_continuation_unavailable"
            );
            assert_eq!(observed.headers.lock().unwrap().len(), 1);
            assert_eq!(observed.requests.lock().unwrap().len(), 1);
        }
    }
}

#[tokio::test]
async fn turn_state_is_exact_and_invalidated_by_in_band_auth_refresh() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::JsonWithHeaders(
            StatusCode::OK,
            json!({"id":"first","output":[]}),
            vec![("x-codex-turn-state", "synthetic-state")],
        ),
        success_reply("second"),
        success_reply("third"),
        Reply::Json(
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"invalid_api_key"}}),
        ),
        success_reply("refreshed"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "relay-refresh-account",
            TokenSet::new(
                "old-access",
                Some("refresh-secret".into()),
                None,
                Some(current_time_ms() + 600_000),
                current_time_ms(),
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let (gateway, _, refresh, _) = spawn_mixed_gateway(
        vec![],
        vec![account(
            "relay-refresh-account",
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
    let client = reqwest::Client::new();
    for state in [
        None,
        Some("synthetic-state"),
        Some("unknown-state"),
        Some("synthetic-state"),
    ] {
        let mut request = client
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .header("x-session-id", "synthetic-session")
            .json(&json!({"model":MODEL,"input":"synthetic input"}));
        if let Some(state) = state {
            request = request.header("x-codex-turn-state", state);
        }
        assert_eq!(request.send().await.unwrap().status(), StatusCode::OK);
    }
    let requests = state.requests.lock().unwrap();
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[0].turn_state, None);
    assert_eq!(requests[1].turn_state.as_deref(), Some("synthetic-state"));
    assert_eq!(requests[2].turn_state, None);
    assert_eq!(requests[3].turn_state.as_deref(), Some("synthetic-state"));
    assert_eq!(requests[4].turn_state, None);
}

async fn setup(
    replies: Vec<Reply>,
) -> (
    TestServer,
    TestServer,
    UpstreamState,
    Arc<Mutex<Vec<UsageEvent>>>,
) {
    let (upstream, state) = spawn_upstream(replies).await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![],
        vec![account(
            "compat-account",
            "synthetic-provider",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        ready_authority("compat-account", "synthetic-access").await,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    (gateway, upstream, state, events)
}

#[tokio::test]
async fn compressed_requests_reach_account_and_compact_endpoints() {
    let (gateway, _upstream, state, _) = setup(vec![]).await;
    let input =
        json!({"model":MODEL,"input":[{"role":"user","content":"synthetic input"}]}).to_string();
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(input.as_bytes()).unwrap();
    for (encoding, bytes) in [
        ("gzip", gzip.finish().unwrap()),
        (
            "zstd",
            zstd::stream::encode_all(input.as_bytes(), 1).unwrap(),
        ),
    ] {
        for path in ["/v1/responses", "/v1/responses/compact"] {
            let response = reqwest::Client::new()
                .post(format!("{}{path}", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .header("content-encoding", encoding)
                .header(CONTENT_TYPE, "application/json")
                .body(bytes.clone())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{encoding}: {path}");
        }
    }
    assert_eq!(state.requests.lock().unwrap().len(), 4);
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .header("content-encoding", "br")
        .body(input)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(state.requests.lock().unwrap().len(), 4);
}

#[tokio::test]
async fn missing_compact_endpoint_uses_one_v2_attempt_with_usage() {
    let (gateway, _upstream, state, events) = setup(vec![
        Reply::Json(StatusCode::NOT_FOUND, json!({"error":{"code":"endpoint_not_found"}})),
        Reply::Stream(vec![
            StreamChunk::Data("data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"compaction\",\"encrypted_content\":\"synthetic\"}}\n\n"),
            StreamChunk::Data("data: {\"type\":\"response.completed\",\"response\":{\"id\":\"synthetic-compact\",\"status\":\"completed\",\"usage\":{\"input_tokens\":42,\"output_tokens\":3}}}\n\n"),
        ]),
    ]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":MODEL,"input":"synthetic input","extension":true}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["object"], "response.compaction");
    assert_eq!(body["output"][0]["encrypted_content"], "synthetic");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].path, "/v1/responses/compact");
    assert_eq!(requests[1].path, "/v1/responses");
    assert_eq!(
        requests[1].body["input"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()["type"],
        "compaction_trigger"
    );
    assert_eq!(requests[1].body["extension"], true);
    assert_eq!(
        events.lock().unwrap().last().unwrap().input_tokens,
        Some(42)
    );
}

#[tokio::test]
async fn incomplete_compaction_is_not_replayed_or_reported_as_success() {
    let (gateway, _upstream, state, events) = setup(vec![
        Reply::Json(StatusCode::METHOD_NOT_ALLOWED, json!({})),
        Reply::Stream(vec![StreamChunk::Data("data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"compaction\",\"encrypted_content\":\"synthetic\"}}\n\n")]),
        success_reply("must-not-run"),
    ]).await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":MODEL,"input":"synthetic input"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "compaction_response_invalid");
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    assert!(!events.lock().unwrap().last().unwrap().success);
}

#[tokio::test]
async fn compact_bridge_preserves_rate_limit_delay_and_stops() {
    let (gateway, _upstream, state, events) = setup(vec![
        Reply::Json(StatusCode::METHOD_NOT_ALLOWED, json!({})),
        Reply::JsonWithHeaders(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"code":"rate_limit_exceeded"}}),
            vec![("retry-after", "120")],
        ),
        success_reply("must-not-run"),
    ])
    .await;
    let started = current_time_ms();
    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses/compact", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":MODEL,"input":"synthetic input"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    let event = events.last().unwrap();
    assert!(!event.success);
    assert!(event
        .retry_at_ms
        .is_some_and(|retry_at| retry_at >= started + 120_000));
}

#[tokio::test]
async fn compressed_errors_use_shared_decoder_without_sending_upstream() {
    let (gateway, _upstream, state, _) = setup(vec![]).await;
    let client = reqwest::Client::new();
    for path in [
        "/v1/responses",
        "/v1/chat/completions",
        "/v1/messages",
        "/v1beta/models/synthetic:generateContent",
    ] {
        let response = client
            .post(format!("{}{path}", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .header("content-encoding", "zstd")
            .body("invalid compressed bytes")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{path}");
        assert_eq!(
            response.headers()["x-zenith-relay-error-category"],
            "request_encoding_invalid",
            "{path}"
        );
    }
    assert!(state.requests.lock().unwrap().is_empty());
}
