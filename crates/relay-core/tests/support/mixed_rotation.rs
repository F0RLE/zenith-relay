//! Real local sockets, shared capacity and repeated mixed-transport waves.

use super::*;

async fn mixed_http(
    State(state): State<UpstreamState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response<Body> {
    let value: Value = serde_json::from_slice(&body).unwrap();
    let image = uri.path().ends_with("/images/generations");
    let reply = if image {
        assert_eq!(value["model"], "gpt-image-2.5-sunburst");
        assert!(value.get("tools").is_none());
        Reply::Json(StatusCode::OK, json!({"data":[{"b64_json":"aW1hZ2U="}]}))
    } else if value["stream"] == true {
        successful_sse_reply()
    } else {
        success_reply("mixed-response")
    };
    let delay = if value["input"].to_string().contains("slow") {
        Duration::from_millis(30)
    } else {
        Duration::from_millis(2)
    };
    upstream(
        State(UpstreamState {
            replies: Arc::new(Mutex::new(VecDeque::from([reply]))),
            delay,
            ..state
        }),
        headers,
        uri,
        body,
    )
    .await
}

async fn mixed_upstream() -> (TestServer, UpstreamState, WebSocketUpstreamState) {
    let http = UpstreamState {
        model_catalog: default_upstream_model_catalog(),
        ..Default::default()
    };
    let websocket = WebSocketUpstreamState::default();
    let router = Router::new()
        .route("/v1/models", get(upstream_models))
        .route("/v1/responses", post(mixed_http))
        .route("/v1/images/generations", post(mixed_http))
        .with_state(http.clone())
        .merge(
            Router::new()
                .route("/v1/responses", get(upstream_websocket))
                .with_state(websocket.clone()),
        );
    (spawn(router).await, http, websocket)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_http_sse_websocket_and_image_waves_release_shared_capacity() {
    run_mixed_waves(12).await;
}

#[tokio::test]
async fn http_to_websocket_continuation_preserves_owner_and_portable_tool_history() {
    for api_source in [false, true] {
        let call = json!({
            "type": "function_call", "call_id": "call_earlier",
            "name": "lookup", "arguments": "{}"
        });
        let (earlier, earlier_http) = spawn_upstream(vec![Reply::Json(
            StatusCode::OK,
            json!({"id": "earlier-response", "status": "completed", "model": MODEL, "output": [call.clone()]}),
        )])
        .await;
        let current_http = UpstreamState {
            replies: Arc::new(Mutex::new(VecDeque::from([success_reply(
                "current-response",
            )]))),
            model_catalog: default_upstream_model_catalog(),
            ..Default::default()
        };
        let current_ws = WebSocketUpstreamState::default();
        let current = spawn(
            Router::new()
                .route("/v1/models", get(upstream_models))
                .route("/v1/responses", post(upstream))
                .with_state(current_http.clone())
                .merge(
                    Router::new()
                        .route("/v1/responses", get(upstream_websocket))
                        .with_state(current_ws.clone()),
                ),
        )
        .await;
        let authority = Arc::new(TokenAuthority::new(4).unwrap());
        let mut sources = Vec::new();
        let mut accounts = Vec::new();
        for (id, upstream) in [("earlier", &earlier), ("current", &current)] {
            if api_source {
                sources.push(source(id, upstream, "synthetic-secret", 100));
            } else {
                register_ready(&authority, id, "synthetic-access").await;
                accounts.push(account(id, id, upstream, 100));
            }
        }
        let (gateway, _, _, _) = spawn_mixed_gateway(
            sources,
            accounts,
            vec![mixed_key(None, None)],
            authority,
            refresh_adapter(),
            Arc::new(PersistenceAdapter::default()),
        )
        .await;
        let runtime = gateway.runtime.as_ref().unwrap();
        assert!(runtime.set_candidate_health("current", CandidateHealth::Unhealthy));
        let first: Value = request(&gateway, false).await.json().await.unwrap();
        assert_eq!(first["id"], "earlier-response");
        assert!(runtime.set_candidate_health("current", CandidateHealth::Healthy));
        assert!(runtime.set_candidate_health("earlier", CandidateHealth::Unhealthy));
        let input = json!([
            call,
            {"type": "function_call_output", "call_id": "call_earlier", "output": "synthetic result"},
            {"role": "user", "content": "continue"}
        ]);
        let client = reqwest::Client::new();
        let url = format!("{}/v1/responses", gateway.base_url);
        let second = client
            .post(&url)
            .bearer_auth(LOCAL_KEY)
            .json(&json!({"model": MODEL, "input": input, "stream": false}))
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::OK);
        let second: Value = second.json().await.unwrap();
        assert_eq!(second["id"], "current-response");
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
        for predecessor in ["current-response", "ws-response"] {
            socket
                .send(ClientWsMessage::Text(
                    json!({
                        "type": "response.create", "model": MODEL,
                        "previous_response_id": predecessor, "input": input
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            let completed = receive_websocket_completion(&mut socket).await;
            assert_eq!(completed["response"]["id"], "ws-response");
        }
        assert_eq!(earlier_http.requests.lock().unwrap().len(), 1);
        assert_eq!(current_http.requests.lock().unwrap().len(), 1);
        let forwarded = current_ws.requests.lock().unwrap();
        assert_eq!(forwarded.len(), 2);
        for (request, predecessor) in forwarded.iter().zip(["current-response", "ws-response"]) {
            assert_eq!(request["previous_response_id"], predecessor);
            assert_eq!(request["input"], input);
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "extended local mixed-transport soak; no provider requests"]
async fn extended_mixed_transport_soak() {
    run_mixed_waves(240).await;
}

async fn run_mixed_waves(waves: usize) {
    use zenith_relay_core::{PoolMemberKind, PoolRoutingMember, PoolRoutingPolicy};

    let (first, first_http, first_ws) = mixed_upstream().await;
    let (second, second_http, second_ws) = mixed_upstream().await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    let mut accounts = Vec::new();
    for (id, upstream) in [("first", &first), ("second", &second)] {
        register_ready(&authority, id, "synthetic-access").await;
        let mut configured = account(id, id, upstream, 100);
        configured.models.push(OFFICIAL_CODEX_MODEL.into());
        accounts.push(configured);
    }
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        accounts,
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap();
    runtime
        .set_pool_routing_policy(
            PoolRoutingPolicy {
                members: ["first", "second"]
                    .map(|id| PoolRoutingMember {
                        kind: PoolMemberKind::Account,
                        id: id.into(),
                        weight: 1,
                        max_concurrency: 4,
                    })
                    .into(),
                ..Default::default()
            },
            3,
        )
        .unwrap();
    let client = reqwest::Client::new();
    let base_url = &gateway.base_url;
    for wave in 0..waves {
        // Healthy waves alternate with a temporarily unavailable member.
        assert!(runtime.set_candidate_health(
            "first",
            if wave % 2 == 0 {
                CandidateHealth::Healthy
            } else {
                CandidateHealth::Unhealthy
            }
        ));
        let responses = (0..24).map(|index| {
            let client = client.clone();
            async move {
                if index % 4 == 2 {
                    let response = client.get(format!("{base_url}/v1/responses"))
                        .bearer_auth(LOCAL_KEY).upgrade().send().await.unwrap();
                    assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
                    let mut socket = response.into_websocket().await.unwrap();
                    socket.send(ClientWsMessage::Text(json!({
                        "type": "response.create", "model": MODEL,
                        "input": format!("mixed websocket {wave}-{index}"),
                    }).to_string())).await.unwrap();
                    assert_eq!(receive_websocket_completion(&mut socket).await["type"], "response.completed");
                } else {
                    let image = index % 4 == 3;
                    let path = if image { "images/generations" } else { "responses" };
                    let body = if image {
                        json!({"model": "gpt-image-2.5-sunburst", "prompt": "synthetic image"})
                    } else {
                        json!({"model": MODEL, "input": format!("slow mixed {wave}-{index}"), "stream": index % 4 == 1})
                    };
                    let response = client.post(format!("{base_url}/v1/{path}"))
                        .bearer_auth(LOCAL_KEY).json(&body).send().await.unwrap();
                    let status = response.status();
                    let body = response.text().await.unwrap();
                    assert_eq!(status, StatusCode::OK, "wave={wave} index={index}: {body}");
                    if image {
                        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["data"][0]["b64_json"], "aW1hZ2U=");
                    } else if index % 4 == 1 {
                        assert!(body.contains("response.completed"));
                    } else {
                        assert_eq!(serde_json::from_str::<Value>(&body).unwrap()["status"], "completed");
                    }
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(15), join_all(responses))
            .await
            .expect("mixed transport wave stalled");
        tokio::time::timeout(Duration::from_secs(2), async {
            while events.lock().unwrap().len() != (wave + 1) * 24
                || runtime
                    .candidate_runtime_order()
                    .iter()
                    .any(|member| member.in_flight != 0)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("mixed wave retained a lease or missed accounting");
    }
    let first_count =
        first_http.requests.lock().unwrap().len() + first_ws.requests.lock().unwrap().len();
    let second_count =
        second_http.requests.lock().unwrap().len() + second_ws.requests.lock().unwrap().len();
    assert_eq!(first_count + second_count, waves * 24);
    assert!(first_count > 0 && second_count > first_count);
    assert!(events.lock().unwrap().iter().all(|event| event.success));
}
