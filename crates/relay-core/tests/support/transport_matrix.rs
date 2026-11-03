use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_transport_concurrency_matrix_balances_and_releases_all_leases() {
    let _guard = LOCAL_SOCKET_LOAD_LOCK.lock().await;
    for requests in [1, 20, 200] {
        assert_sse_concurrency(requests).await;
    }
}

async fn assert_sse_concurrency(requests: usize) {
    // Keep every lease live until every request has selected a route. Without
    // this gate, a fast local SSE response can finish while later client tasks
    // are still opening their connections, so the assertion measures arrival
    // timing instead of concurrent routing.
    let request_barrier = Arc::new(Barrier::new(requests + 1));
    let (first_upstream, first_state) = spawn_gated_upstream(
        vec![successful_sse_reply(); requests],
        request_barrier.clone(),
    )
    .await;
    let (second_upstream, second_state) = spawn_gated_upstream(
        vec![successful_sse_reply(); requests],
        request_barrier.clone(),
    )
    .await;
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
    let url = format!("{}/v1/responses", gateway.base_url);
    let (completed, _) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(
            join_all((0..requests).map(|index| {
                let client = client.clone();
                let url = url.clone();
                async move {
                    let response = client
                        .post(url)
                        .bearer_auth(LOCAL_KEY)
                        .json(&json!({
                            "model": MODEL,
                            "input": format!("parallel SSE chat {index}"),
                            "stream": true
                        }))
                        .send()
                        .await
                        .unwrap();
                    response.status() == StatusCode::OK
                        && response
                            .text()
                            .await
                            .unwrap()
                            .contains("response.completed")
                }
            })),
            request_barrier.wait(),
        )
    })
    .await
    .expect("parallel SSE requests did not reach the upstream barrier");

    assert!(completed.into_iter().all(|completed| completed));
    assert_transport_matrix_state(
        requests,
        &first_state.requests,
        &second_state.requests,
        &events,
        &gateway,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websocket_transport_concurrency_matrix_balances_and_releases_all_leases() {
    let _guard = LOCAL_SOCKET_LOAD_LOCK.lock().await;
    for requests in [1, 20, 200] {
        assert_websocket_concurrency(requests, Duration::ZERO).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_websocket_fallback_balances_without_duplicate_generations() {
    let _guard = LOCAL_SOCKET_LOAD_LOCK.lock().await;
    assert_websocket_concurrency(20, Duration::from_secs(3)).await;
}

async fn spawn_matrix_upstream(
    barrier: Arc<Barrier>,
    handshake_delay: Duration,
) -> (TestServer, WebSocketUpstreamState) {
    let state = WebSocketUpstreamState {
        behavior: WebSocketBehavior::GatedSuccess(barrier),
        model_catalog: default_upstream_model_catalog(),
        ..Default::default()
    };
    // A slow, unsent upgrade may legitimately use HTTP on a busy runner.
    // Both transports share the same generation gate and request accounting.
    let app = Router::new()
        .route("/v1/models", get(upstream_websocket_models))
        .route(
            "/v1/responses",
            get(move |state, headers, websocket| async move {
                if !handshake_delay.is_zero() {
                    tokio::time::sleep(handshake_delay).await;
                }
                upstream_websocket(state, headers, websocket).await
            })
            .post(matrix_http),
        )
        .with_state(state.clone());
    (spawn(app).await, state)
}

async fn matrix_http(
    State(state): State<WebSocketUpstreamState>,
    headers: HeaderMap,
    uri: Uri,
    body: Bytes,
) -> Response<Body> {
    let WebSocketBehavior::GatedSuccess(barrier) = state.behavior else {
        panic!("matrix upstream requires a shared generation gate");
    };
    state
        .requests
        .lock()
        .unwrap()
        .push(serde_json::from_slice(&body).unwrap());
    upstream(
        State(UpstreamState {
            replies: Arc::new(Mutex::new(VecDeque::from([successful_sse_reply()]))),
            request_barrier: Some(barrier),
            ..Default::default()
        }),
        headers,
        uri,
        body,
    )
    .await
}

async fn assert_websocket_concurrency(requests: usize, handshake_delay: Duration) {
    // Match the SSE matrix: keep every lease live until all requests have
    // selected a route. Fast completions must not turn arrival timing into
    // an apparent distribution failure on shared CI runners.
    let request_barrier = Arc::new(Barrier::new(requests + 1));
    let (first_upstream, first_state) =
        spawn_matrix_upstream(request_barrier.clone(), handshake_delay).await;
    let (second_upstream, second_state) =
        spawn_matrix_upstream(request_barrier.clone(), handshake_delay).await;
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
    let url = format!("{}/v1/responses", gateway.base_url);
    // Allow the largest burst time to open its upgrades on a shared runner.
    let queue_timeout =
        Duration::from_secs((u64::try_from(requests).unwrap_or(u64::MAX) / 10).saturating_add(5));
    // Each client owns a cooperative budget for socket progress. JoinSet also
    // cancels outstanding clients when this fixture fails.
    let mut clients = tokio::task::JoinSet::new();
    for index in 0..requests {
        let client = client.clone();
        let url = url.clone();
        clients.spawn(async move {
            let upgraded = client
                .get(url)
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
                        "input": format!("parallel chat {index}")
                    })
                    .to_string(),
                ))
                .await
                .unwrap();
            receive_websocket_completion_with_timeout(&mut socket, queue_timeout).await["type"]
                == "response.completed"
        });
    }
    let requests_complete = async {
        let mut completed = 0;
        while let Some(result) = clients.join_next().await {
            assert!(result.expect("websocket client task failed"));
            completed += 1;
        }
        completed
    };
    let (completed, _) = tokio::time::timeout(queue_timeout, async {
        tokio::join!(requests_complete, request_barrier.wait())
    })
    .await
    .expect("parallel websocket requests timed out");

    assert_eq!(completed, requests);
    assert_transport_matrix_state(
        requests,
        &first_state.requests,
        &second_state.requests,
        &events,
        &gateway,
    )
    .await;
    assert!(events.lock().unwrap().iter().all(|event| {
        event.success && event.transport == zenith_relay_core::UsageTransport::Websocket
    }));
    let first_requests = first_state.requests.lock().unwrap();
    let second_requests = second_state.requests.lock().unwrap();
    let inputs: BTreeSet<_> = first_requests
        .iter()
        .chain(second_requests.iter())
        .map(|request| {
            if !handshake_delay.is_zero() {
                assert!(
                    request.get("type").is_none(),
                    "slow upgrade dispatched a generation"
                );
            }
            assert!(!request["input"].is_null());
            request["input"].to_string()
        })
        .collect();
    assert_eq!(inputs.len(), requests, "a generation was replayed");
}

async fn assert_transport_matrix_state<T, U>(
    requests: usize,
    first_requests: &Arc<Mutex<Vec<T>>>,
    second_requests: &Arc<Mutex<Vec<T>>>,
    events: &Arc<Mutex<Vec<U>>>,
    gateway: &TestServer,
) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while events.lock().unwrap().len() != requests {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("transport usage events did not finish");
    let first_requests = first_requests.lock().unwrap().len();
    let second_requests = second_requests.lock().unwrap().len();
    assert_eq!(first_requests + second_requests, requests);
    assert!(
        first_requests.abs_diff(second_requests) <= requests.max(20) / 20,
        "parallel routing was unexpectedly skewed: {first_requests}/{second_requests}"
    );
    tokio::time::timeout(Duration::from_secs(2), async {
        while !gateway
            .runtime
            .as_ref()
            .unwrap()
            .candidate_runtime_order()
            .iter()
            .all(|candidate| candidate.in_flight == 0)
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("transport candidate leases did not finish");
}
