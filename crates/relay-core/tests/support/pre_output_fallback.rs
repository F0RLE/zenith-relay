use super::*;

#[tokio::test]
async fn generic_five_xx_stops_replay_but_does_not_block_an_independent_request() {
    for status in [StatusCode::BAD_GATEWAY, StatusCode::SERVICE_UNAVAILABLE] {
        assert_server_error_fallback(status).await;
    }
}

async fn assert_server_error_fallback(status: StatusCode) {
    let (source_a, state_a) =
        spawn_upstream("source-a-key", vec![status_reply(status, "loser", None)]).await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![
            response_reply("resp-b-1", "winner"),
            response_reply("resp-b-2", "winner"),
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway_with_options(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            model_metadata_catalog: None,
            max_retry_candidates: 3,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let first = request(&gateway, false).await;
    assert_eq!(first.status(), status);
    let _ = first.bytes().await.unwrap();
    assert!(state_b.requests.lock().unwrap().is_empty());
    assert_eq!(
        request(&gateway, false)
            .await
            .json::<Value>()
            .await
            .unwrap()["id"],
        "resp-b-1"
    );

    let a = state_a.requests.lock().unwrap();
    let b = state_b.requests.lock().unwrap();
    assert_eq!(a.len(), 1, "5xx source should remain in cooldown");
    assert_eq!(b.len(), 1);
    assert_eq!(a[0].authorization.as_deref(), Some("Bearer source-a-key"));
    assert_eq!(b[0].authorization.as_deref(), Some("Bearer source-b-key"));
    assert!(!a[0].authorization.as_deref().unwrap().contains(LOCAL_KEY));
    drop(a);
    drop(b);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        (
            events[0].attempt,
            events[0].source_id.as_str(),
            events[0].success
        ),
        (1, "source-a", false)
    );
    assert_eq!(
        (
            events[1].attempt,
            events[1].source_id.as_str(),
            events[1].success
        ),
        (1, "source-b", true)
    );
    assert_ne!(events[0].request_id, events[1].request_id);
}

#[tokio::test]
async fn function_tool_output_stays_on_its_creator_after_a_transient_failure() {
    assert_tool_output_owner_after_failure(false).await;
}

#[tokio::test]
async fn successful_tool_output_without_predecessor_does_not_create_portable_history() {
    assert_tool_output_owner_after_failure(true).await;
}

async fn assert_tool_output_owner_after_failure(complete_tool_turn: bool) {
    let call_id = "call_stateful_01";
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![
            Reply::Json {
                status: StatusCode::OK,
                body: json!({
                    "id": "resp-tool-owner",
                    "object": "response",
                    "model": MODEL,
                    "output": [{
                        "type": "function_call",
                        "id": "fc_stateful_01",
                        "call_id": call_id,
                        "name": "run_command",
                        "arguments": "{\"command\":\"pwd\"}"
                    }],
                    "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}
                }),
                cache_control: "owner",
                retry_after: None,
            },
            status_reply(StatusCode::SERVICE_UNAVAILABLE, "owner", None),
        ],
    )
    .await;
    if complete_tool_turn {
        state_a
            .replies
            .lock()
            .unwrap()
            .insert(1, response_reply("resp-tool-result", "owner"));
    }
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![response_reply("resp-wrong-owner", "fallback")],
    )
    .await;
    let (gateway, _) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let first = request(&gateway, false).await;
    assert_eq!(first.status(), StatusCode::OK);

    let mut continuation = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [{
                "type": "function_call_output",
                "call_id": call_id,
                "output": "C:\\workspace"
            }]
        }))
        .send()
        .await
        .unwrap();

    if complete_tool_turn {
        assert_eq!(continuation.status(), StatusCode::OK);
        let response: Value = continuation.json().await.unwrap();
        continuation = reqwest::Client::new()
            .post(format!("{}/v1/responses", gateway.base_url))
            .bearer_auth(LOCAL_KEY)
            .json(&json!({
                "model": MODEL,
                "previous_response_id": response["id"],
                "input": "continue with the original constraints"
            }))
            .send()
            .await
            .unwrap();
    }
    assert_eq!(continuation.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        state_a.requests.lock().unwrap().len(),
        2 + usize::from(complete_tool_turn)
    );
    assert!(state_b.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn orphaned_function_tool_output_is_rejected_before_pool_selection() {
    let (upstream, state) = spawn_upstream(
        "source-key",
        vec![response_reply("resp-continued", "unused")],
    )
    .await;
    let (gateway, _) = spawn_gateway(
        vec![source("source", &upstream, "source-key", &[MODEL], 0)],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = reqwest::Client::new()
        .post(format!("{}/v1/responses", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({
            "model": MODEL,
            "input": [{
                "type": "function_call_output",
                "call_id": "call_unknown",
                "output": "result"
            }]
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "response_continuation_unavailable"
    );
    assert!(state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn shared_endpoint_stabilizers_are_retried_before_last_reserve() {
    let (shared_endpoint, shared_state) = spawn_upstream(
        "shared-key",
        vec![
            overload_reply("stabilizer-down", None),
            response_reply("stabilizer-response", "stabilizer"),
        ],
    )
    .await;
    let (independent_endpoint, independent_state) = spawn_upstream(
        "independent-key",
        vec![response_reply("must-not-run", "last-reserve")],
    )
    .await;
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured_events = events.clone();
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            vec![
                source("stabilizer-a", &shared_endpoint, "shared-key", &[MODEL], 20),
                source("stabilizer-b", &shared_endpoint, "shared-key", &[MODEL], 10),
                source(
                    "last-reserve",
                    &independent_endpoint,
                    "independent-key",
                    &[MODEL],
                    -1_000_000,
                ),
            ],
            vec![local_key("key", LOCAL_KEY, None)],
            GatewayRuntimeOptions {
                pool_routing: Some(ordered_policy(&[
                    "stabilizer-a",
                    "stabilizer-b",
                    "last-reserve",
                ])),
                max_retry_candidates: 3,
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(move |event| captured_events.lock().unwrap().push(event)),
        )
        .unwrap(),
    );
    let gateway = spawn(gateway::router(runtime.clone())).await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CACHE_CONTROL], "stabilizer");
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "stabilizer-response"
    );
    assert_eq!(shared_state.requests.lock().unwrap().len(), 2);
    assert!(independent_state.requests.lock().unwrap().is_empty());
    assert_eq!(events.lock().unwrap().len(), 2);

    let runtime_order = runtime.candidate_runtime_order();
    let shared = ["stabilizer-a", "stabilizer-b"]
        .into_iter()
        .map(|candidate_id| {
            runtime_order
                .iter()
                .find(|candidate| candidate.candidate_id == candidate_id)
                .unwrap()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        shared
            .iter()
            .filter(|candidate| !candidate.available)
            .count(),
        1
    );
    assert_eq!(
        shared
            .iter()
            .filter(|candidate| candidate.next_retry_at_ms.is_none())
            .count(),
        1
    );
    assert!(
        runtime_order
            .iter()
            .find(|candidate| candidate.candidate_id == "last-reserve")
            .unwrap()
            .available
    );
}

#[tokio::test]
async fn oversized_five_xx_body_does_not_authorize_another_generation() {
    let (shared_endpoint, shared_state) = spawn_upstream(
        "shared-key",
        vec![
            Reply::Oversized {
                status: StatusCode::SERVICE_UNAVAILABLE,
                cache_control: "stabilizer-down",
            },
            response_reply("stabilizer-response", "stabilizer"),
        ],
    )
    .await;
    let (independent_endpoint, independent_state) = spawn_upstream(
        "independent-key",
        vec![response_reply("must-not-run", "last-reserve")],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("stabilizer-a", &shared_endpoint, "shared-key", &[MODEL], 20),
            source("stabilizer-b", &shared_endpoint, "shared-key", &[MODEL], 0),
            source(
                "last-reserve",
                &independent_endpoint,
                "independent-key",
                &[MODEL],
                -1_000_000,
            ),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let _ = response.bytes().await.unwrap();

    let shared_requests = shared_state.requests.lock().unwrap();
    assert_eq!(
        shared_requests.len(),
        1,
        "an unreadable 5xx cannot prove rejection before execution"
    );
    assert_eq!(
        shared_requests[0].authorization.as_deref(),
        Some("Bearer shared-key")
    );
    assert!(independent_state.requests.lock().unwrap().is_empty());
    assert_eq!(events.lock().unwrap().len(), 1);
}
