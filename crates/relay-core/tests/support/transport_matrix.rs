use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_transport_concurrency_matrix_balances_and_releases_all_leases() {
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
    for requests in [1, 20, 200] {
        assert_websocket_concurrency(requests).await;
    }
}

async fn assert_websocket_concurrency(requests: usize) {
    // Match the SSE matrix: keep every lease live until all requests have
    // selected a route. Fast completions must not turn arrival timing into
    // an apparent distribution failure on shared CI runners.
    let request_barrier = Arc::new(Barrier::new(requests + 1));
    let (first_upstream, first_state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::GatedSuccess(request_barrier.clone()),
    )
    .await;
    let (second_upstream, second_state) = spawn_websocket_upstream_with_behavior(
        WebSocketBehavior::GatedSuccess(request_barrier.clone()),
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
    // Allow the largest burst time to open its upgrades on a shared runner.
    let queue_timeout =
        Duration::from_secs((u64::try_from(requests).unwrap_or(u64::MAX) / 10).saturating_add(5));
    let requests_complete = join_all((0..requests).map(|index| {
        let client = client.clone();
        let url = url.clone();
        async move {
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
        }
    }));
    let (completed, _) = tokio::time::timeout(queue_timeout, async {
        tokio::join!(requests_complete, request_barrier.wait())
    })
    .await
    .expect("parallel websocket requests timed out");

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
