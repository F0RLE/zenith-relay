use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};

#[tokio::test]
async fn websocket_bps_continuation_preserves_tool_history() {
    for (full_history, history_bytes, description_bytes, bps_count, colliding_hint) in [
        (true, 32, 32, 2, false),
        (false, 1_750_000, 400_000, 2, false),
        (false, 2_200_000, 32, 2, false),
        (false, 32, 32, 1, false),
        (false, 32, 32, 2, false),
        (false, 32, 32, 2, true),
    ] {
        let bps = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        bps.state.stream_generations.store(true, Ordering::SeqCst);
        bps.state
            .strict_request_schema
            .store(true, Ordering::SeqCst);
        let mut runtime = retry_admission::pool_runtime(&bps, bps_count, false).await;
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let runtime = Arc::new(runtime);
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let mut socket = retry_admission::bounded_client()
            .get(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .upgrade()
            .send()
            .await
            .unwrap()
            .into_websocket()
            .await
            .unwrap();
        let mut history = vec![json!({"role":"user","content":"x".repeat(history_bytes)})];
        for turn in 0..3 {
            let output = if turn < 2 {
                json!([{
                    "type":"function_call","id":format!("fc_synthetic_{turn}"),
                    "name":"run_officejs","call_id":format!("call_synthetic_{turn}"),
                    "status":"completed",
                    "arguments":json!({
                        "references":["lookup"],
                        "code":json!({"path":format!("synthetic-{turn}")}).to_string()
                    }).to_string()
                }])
            } else {
                json!([{"type":"message","role":"assistant","content":[{
                    "type":"output_text","text":"Synthetic final answer"
                }]}])
            };
            *crate::poison::mutex(&bps.state.generation_body) = json!({
                "id":format!("synthetic-bps-continuation-{turn}"),
                "model":"gpt-test","status":"completed","output":output
            });
            let mut request = json!({
                "type":"response.create","model":"gpt-test",
                "input":if full_history || turn == 0 {
                    json!(history)
                } else {
                    json!([history.last().unwrap()])
                },
                "tools":[{"type":"function","name":"lookup",
                    "description":"d".repeat(description_bytes),"parameters":{
                    "type":"object","properties":{"path":{"type":"string"}},
                    "required":["path"],"additionalProperties":false
                }}]
            });
            if !full_history && turn > 0 {
                request["previous_response_id"] =
                    json!(format!("synthetic-bps-continuation-{}", turn - 1));
            }
            if colliding_hint && turn > 0 {
                let now = crate::unix_time_ms();
                let key = runtime
                    .authenticate(Some(&axum::http::HeaderValue::from_static(
                        "Bearer local-secret",
                    )))
                    .unwrap();
                let previous = request["previous_response_id"].as_str().unwrap();
                let affinity = runtime.response_affinity_key(Some(previous)).unwrap();
                let owner = runtime.response_affinity_candidate(&affinity, now).unwrap();
                let other = runtime
                    .configured_executor_routes(&key, "gpt-test", &[WireApi::Responses], false)
                    .into_iter()
                    .find(|route| route.candidate_id != owner)
                    .unwrap();
                runtime.bind_tool_call_route_affinity(
                    &key.id,
                    history.last().unwrap()["call_id"].as_str().unwrap(),
                    &other,
                    now,
                );
            }
            socket
                .send(Message::Text(request.to_string()))
                .await
                .unwrap();
            let terminal = tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(message) = socket.next().await {
                    if let Message::Text(text) = message.unwrap() {
                        let event: Value = serde_json::from_str(&text).unwrap();
                        if event["type"] == "response.completed" || event["type"] == "error" {
                            return event;
                        }
                    }
                }
                panic!("BPS continuation socket closed without a terminal response");
            })
            .await
            .expect("BPS continuation must finish");
            assert_eq!(
                terminal["type"], "response.completed",
                "full_history={full_history}, history_bytes={history_bytes}, bps_count={bps_count}, colliding_hint={colliding_hint}, turn={turn}: {terminal}"
            );
            assert_eq!(
                terminal["response"]["id"],
                format!("synthetic-bps-continuation-{turn}")
            );
            let sent = crate::poison::mutex(&bps.state.generation_bodies)[turn].clone();
            assert!(sent.get("previous_response_id").is_none());
            for prior in 0..turn {
                assert!(
                    sent["input"].as_array().unwrap().iter().any(|item| {
                        item["type"] == "function_call_output"
                            && item["call_id"] == format!("call_synthetic_{prior}")
                            && item["output"] == format!("Synthetic result {prior}")
                    }),
                    "missing tool result {prior}"
                );
            }
            let response_output = terminal["response"]["output"].as_array().unwrap();
            history.extend(response_output.iter().cloned());
            if turn < 2 {
                let call = &response_output[0];
                assert_eq!(call["name"], "lookup");
                assert_eq!(call["call_id"], format!("call_synthetic_{turn}"));
                assert_eq!(
                    serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
                    json!({"path":format!("synthetic-{turn}")})
                );
                history.push(json!({
                    "type":"function_call_output","call_id":call["call_id"],
                    "output":format!("Synthetic result {turn}")
                }));
            }
        }
        assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 3);
        let recorded = crate::poison::mutex(&events);
        assert_eq!(recorded.len(), 3);
        assert!(
            recorded
                .iter()
                .all(|event| { event.success && event.candidate_id == recorded[0].candidate_id }),
            "a supported continuation must not rotate away from its owner"
        );
        task.abort();
    }
}

#[tokio::test]
async fn bps_missing_replay_reports_local_state_loss_without_a_provider_send() {
    for stream in [false, true] {
        let bps = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        let mut runtime = retry_admission::pool_runtime(&bps, 2, false).await;
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let runtime = Arc::new(runtime);
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let client = retry_admission::bounded_client();
        let first = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"Synthetic first turn"}))
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        let first: Value = first.json().await.unwrap();
        *crate::poison::mutex(&runtime.native_responses_replay_store) =
            crate::NativeResponsesReplayStore::default();
        let response = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({
                "model":"gpt-test","previous_response_id":first["id"],
                "input":"Synthetic next turn","stream":stream
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            body["error"]["code"], "adapter_continuation_missing",
            "{body}"
        );
        assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 1);
        let recorded = crate::poison::mutex(&events);
        assert_eq!(recorded.len(), 2);
        let rejection = recorded.last().unwrap();
        assert_eq!(rejection.attempt, 0);
        assert_eq!(rejection.error_origin(), Some(crate::ErrorOrigin::Relay));
        assert_eq!(
            rejection.error_category.as_deref(),
            Some("adapter_continuation_missing")
        );
        assert!(rejection.candidate_id.is_none());
        assert!(rejection.input_tokens.is_none());
        assert!(rejection.output_tokens.is_none());
        task.abort();
    }
}

#[tokio::test]
async fn websocket_bps_replay_budget_loss_allows_full_history_recovery() {
    let bps = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let mut runtime = retry_admission::pool_runtime(&bps, 1, false).await;
    let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
    let captured = events.clone();
    runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
    *crate::poison::mutex(&runtime.native_responses_replay_store) =
        crate::NativeResponsesReplayStore::with_limits(4, 60_000, 256, 1_024);
    let (base, task) = recovery::gateway(Arc::new(runtime)).await;
    let client = retry_admission::bounded_client();
    let initial_input = json!([{"role":"user","content":"x".repeat(2_048)}]);
    let first = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":initial_input}))
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let first: Value = first.json().await.unwrap();
    let mut history = initial_input.as_array().unwrap().clone();
    history.extend(first["output"].as_array().unwrap().iter().cloned());
    history.push(json!({"role":"user","content":"Synthetic next turn"}));

    for full_history in [false, true] {
        // Match a client's explicit recovery on a fresh connection. Relay
        // itself must not retry the orphan delta or invent the missing input.
        let mut socket = client
            .get(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .upgrade()
            .send()
            .await
            .unwrap()
            .into_websocket()
            .await
            .unwrap();
        let mut request = json!({
            "type":"response.create", "model":"gpt-test", "stream_id":"main",
            "input":if full_history { json!(history) } else { json!([history.last().unwrap()]) }
        });
        if !full_history {
            request["previous_response_id"] = first["id"].clone();
        }
        socket
            .send(Message::Text(request.to_string()))
            .await
            .unwrap();
        let terminal = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(message) = socket.next().await {
                if let Message::Text(text) = message.unwrap() {
                    let event: Value = serde_json::from_str(&text).unwrap();
                    if event["type"] == "response.completed" || event["type"] == "error" {
                        return event;
                    }
                }
            }
            panic!("missing a terminal response");
        })
        .await
        .unwrap();
        assert_eq!(terminal["stream_id"], "main");
        if full_history {
            assert_eq!(terminal["type"], "response.completed", "{terminal}");
        } else {
            assert_eq!(terminal["type"], "error", "{terminal}");
            assert_eq!(terminal["error"]["code"], "previous_response_not_found");
            assert_eq!(terminal["error"]["param"], "previous_response_id");
            assert_eq!(terminal["error"]["zenith_relay"]["origin"], "relay");
            assert_eq!(
                terminal["error"]["zenith_relay"]["category"],
                "adapter_continuation_missing"
            );
            assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 1);
        }
    }
    assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 2);
    let recorded = crate::poison::mutex(&events);
    assert_eq!(recorded.len(), 3);
    assert!(recorded[0].success && recorded[2].success);
    assert_eq!(recorded[1].attempt, 0);
    assert_eq!(recorded[1].error_origin(), Some(crate::ErrorOrigin::Relay));
    assert!(recorded[1].input_tokens.is_none());
    task.abort();
}

async fn bps_only_runtime(bps: &AccessServer, native: &AccessServer) -> Arc<GatewayRuntime> {
    let mut runtime = retry_admission::pool_runtime(bps, 1, true).await;
    runtime
        .chatgpt_accounts
        .get_mut("synthetic-account-1")
        .unwrap()
        .responses_url = native.responses_url.clone();
    {
        let mut scheduler = runtime.lock_scheduler();
        let mut excluded = scheduler.candidate("synthetic-account-1").unwrap().clone();
        excluded.model_rules.excluded.insert("gpt-test".into());
        excluded.priority = 1_000;
        scheduler.upsert(excluded);
    }
    let affinity = runtime
        .prompt_affinity_key("key-1", "gpt-test", Some("synthetic-cache"), None)
        .unwrap();
    runtime.bind_prompt_affinity(
        Some(&affinity),
        "synthetic-account-1",
        crate::unix_time_ms(),
    );
    Arc::new(runtime)
}

#[tokio::test]
async fn responses_routes_a_bps_only_model_without_sending_to_the_excluded_native_account() {
    for (stream, lite) in [(false, false), (true, false), (false, true), (true, true)] {
        let bps = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        let native = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        bps.state.stream_generations.store(stream, Ordering::SeqCst);
        bps.state
            .strict_request_schema
            .store(true, Ordering::SeqCst);
        *crate::poison::mutex(&bps.state.generation_body) = json!({
            "id":"synthetic-bps-response","status":"completed","output":[{
                "type":"message","role":"assistant","content":[{
                    "type":"output_text","text":"Synthetic BPS answer"
                }]
            }]
        });
        let runtime = bps_only_runtime(&bps, &native).await;
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let mut request = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret");
        if lite {
            request = request.header("x-openai-internal-codex-responses-lite", "true");
        }
        let response = request
            .json(&json!({
                "model":"gpt-test","input":"Synthetic request","stream":stream,
                "prompt_cache_key":"synthetic-cache","parallel_tool_calls":true
            }))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("Synthetic BPS answer"), "{body}");
        if stream {
            assert!(body.contains("response.completed"), "{body}");
        }
        assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 1);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        assert!(
            runtime
                .lock_scheduler()
                .candidate("synthetic-account-1")
                .unwrap()
                .enabled
        );
        task.abort();
    }
}

#[tokio::test]
async fn websocket_routes_a_bps_only_model_through_http_without_trying_native_responses() {
    let bps = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let native = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    bps.state.stream_generations.store(true, Ordering::SeqCst);
    bps.state
        .strict_request_schema
        .store(true, Ordering::SeqCst);
    let mut runtime = bps_only_runtime(&bps, &native).await;
    let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
    Arc::get_mut(&mut runtime).unwrap().diagnostics_enabled = || true;
    let captured = events.clone();
    Arc::get_mut(&mut runtime).unwrap().usage = Arc::new(move |event| {
        crate::poison::mutex(&captured).push(event);
    });
    let (base, task) = recovery::gateway(runtime.clone()).await;
    let mut socket = retry_admission::bounded_client()
        .get(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .upgrade()
        .send()
        .await
        .unwrap()
        .into_websocket()
        .await
        .unwrap();
    for turn in 0..3 {
        if turn > 0 {
            if turn == 2 {
                bps.reply_with(
                    StatusCode::OK,
                    json!({"allowed":true,"model_catalog":{"models":[]}}),
                );
            }
            crate::poison::write(
                &runtime.chatgpt_accounts["synthetic-account-0"].basis_points_access,
            )
            .as_mut()
            .unwrap()
            .expire();
        }
        let payload = json!({
            "type":"response.create","model":"gpt-test","input":"Synthetic request",
            "prompt_cache_key":"synthetic-cache"
        })
        .to_string();
        let message = if turn == 1 {
            Message::Binary(payload.into_bytes().into())
        } else {
            Message::Text(payload)
        };
        socket.send(message).await.unwrap();
        let terminal = tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(message) = socket.next().await {
                if let Message::Text(text) = message.unwrap() {
                    let event: Value = serde_json::from_str(&text).unwrap();
                    if event["type"] == "response.completed" || event["type"] == "error" {
                        return event;
                    }
                }
            }
            panic!("socket closed without a terminal BPS response");
        })
        .await
        .expect("BPS-only WebSocket request must finish without a native upgrade");
        if turn == 2 {
            assert_eq!(terminal["type"], "error", "{terminal}");
            assert_eq!(terminal["error"]["code"], "model_not_found");
        } else {
            assert_eq!(terminal["type"], "response.completed", "{terminal}");
            assert_eq!(terminal["response"]["status"], "completed");
        }
    }
    assert_eq!(bps.state.access_requests.load(Ordering::SeqCst), 3);
    assert_eq!(bps.state.generation_requests.load(Ordering::SeqCst), 2);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    let recorded = crate::poison::mutex(&events);
    assert_eq!(recorded.len(), 2);
    for (index, event) in recorded.iter().enumerate() {
        let context = event
            .routing
            .as_ref()
            .unwrap()
            .cache_context
            .as_ref()
            .unwrap();
        assert_eq!(
            context.baseline,
            if index == 0 {
                crate::usage::CacheContextBaseline::FirstObservation
            } else {
                crate::usage::CacheContextBaseline::CompletedRequest
            }
        );
        assert!(context.provider.is_none());
    }
    for event in recorded.iter() {
        assert_eq!(event.transport, crate::UsageTransport::Websocket);
        let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
        assert_eq!(transport.upstream, crate::UsageTransport::Http);
        assert_eq!(transport.mode, crate::usage::TransportMode::BasisPoints);
        assert_eq!(
            transport.fallback_reason,
            Some(crate::usage::TransportFallbackReason::BasisPointsHttp)
        );
        assert_eq!(
            transport.response_format,
            Some(crate::usage::TransportResponseFormat::Sse)
        );
        assert_eq!(transport.handshake_ms, None);
    }
    drop(recorded);
    drop(socket);
    task.abort();
}
