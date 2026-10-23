use super::*;

mod batch;
mod identity;
mod lookup_recovery;
mod streaming;
mod text_only;

fn catalog_request(stream: bool) -> Value {
    json!({"model":"gpt-test","input":"Inspect the synthetic file","stream":stream,
    "relay_bps_tool_catalog":"discovery","tools":[{
        "type":"function","name":"lookup","parameters":{"type":"object","properties":{"path":{"type":"string","minLength":1}},"required":["path"],"additionalProperties":false}
    }]})
}

fn tool_response(name: &str, call_id: &str, arguments: Value) -> Value {
    json!({"id":format!("resp_{call_id}"),"object":"response","model":"gpt-test","status":"completed","output":[{
        "id":format!("fc_{call_id}"),"type":"function_call","name":"run_officejs","status":"completed","call_id":call_id,
        "arguments":json!({"references":[name],"code":arguments.to_string()}).to_string()
    }],"usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110}})
}

fn final_response() -> Value {
    json!({"id":"resp_synthetic_final","object":"response","model":"gpt-test","status":"completed","output":[
        {"type":"message","role":"assistant","content":[{"type":"output_text","text":"Ready"}]}
    ],"usage":{"input_tokens":120,"output_tokens":5,"total_tokens":125}})
}

#[tokio::test]
async fn catalog_discovery_uses_one_budget_and_preserves_client_tool_roundtrip() {
    for (stream, batched, debug) in [
        (false, false, false),
        (true, false, false),
        (false, true, false),
        (true, true, false),
        (false, false, true),
        (true, false, true),
        (false, true, true),
        (true, true, true),
    ] {
        let mut request = catalog_request(stream);
        request["prompt_cache_key"] = json!("synthetic-discovery-cache");
        request["tools"].as_array_mut().unwrap().push(json!({
            "type":"function","name":"later","description":"Previously discovered but not called",
            "parameters":{"type":"object","properties":{"value":{"type":"string"}},"required":["value"]}
        }));
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        server
            .state
            .strict_request_schema
            .store(true, Ordering::SeqCst);
        crate::poison::mutex(&server.state.generation_replies).extend([
            (
                StatusCode::OK,
                tool_response(
                    "relay.catalog",
                    "call_catalog",
                    json!({"names":["lookup", "later"]}),
                ),
            ),
            (
                StatusCode::OK,
                if batched {
                    batch::batch_response()
                } else {
                    tool_response("lookup", "call_client", json!({"path":"synthetic.txt"}))
                },
            ),
        ]);
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.diagnostics_enabled = if debug { || true } else { || false };
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let runtime = Arc::new(runtime);
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request)
            .send()
            .await
            .unwrap();
        let status = response.status();
        let bytes = response.bytes().await.unwrap();
        assert_eq!(
            status,
            StatusCode::OK,
            "stream={stream}, generations={}, modes={:?}, response={}",
            server.state.generation_requests.load(Ordering::SeqCst),
            crate::poison::mutex(&events)
                .iter()
                .map(|event| (
                    event.attempt,
                    event
                        .tool_use
                        .basis_points_catalog
                        .as_ref()
                        .map(|catalog| (&catalog.mode, catalog.discovery_rounds))
                ))
                .collect::<Vec<_>>(),
            String::from_utf8_lossy(&bytes)
        );
        let body: Value = if stream {
            let mut pending = bytes.to_vec();
            let mut completed = None;
            while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
                if let Ok(event) =
                    serde_json::from_slice::<Value>(&crate::protocol::sse_data(&frame))
                {
                    if event["type"] == "response.completed" {
                        completed = Some(event["response"].clone());
                    }
                }
            }
            completed.expect("the client receives an authoritative completed event")
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        assert_eq!(body["output"][0]["name"], "lookup");
        assert_eq!(
            body["output"].as_array().unwrap().len(),
            if batched { 2 } else { 1 }
        );
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
        {
            let events = crate::poison::mutex(&events);
            assert_eq!(events.len(), 2);
            assert_eq!(events[0].request_id, events[1].request_id);
            assert_eq!(events[0].attempt, 1);
            assert_eq!(events[1].attempt, 2);
            assert_eq!(events[0].input_tokens, Some(100));
            assert_eq!(events[1].input_tokens, Some(100));
            assert_eq!(
                events[0].routing.as_ref().unwrap().attempt_outcome,
                Some(crate::AttemptOutcome::ToolDiscovery)
            );
            assert!(events[1].success);
            let sent = crate::poison::mutex(&server.state.generation_bodies);
            for (event, body) in events.iter().zip(sent.iter()) {
                let context = event.routing.as_ref().unwrap().cache_context.as_ref();
                assert_eq!(context.is_some(), debug);
                if let Some(context) = context {
                    assert_eq!(
                        context.upstream_history.input_items,
                        Some(body["input"].as_array().unwrap().len() as u32)
                    );
                }
            }
            if stream {
                assert!(events[1].ttft_ms.is_some());
            } else {
                // Buffered discovery does not fabricate token timestamps.
                assert_eq!(events[1].ttft_ms, None);
            }
            let catalog = events[1].tool_use.basis_points_catalog.as_ref();
            assert_eq!(catalog.is_some(), debug);
            assert_eq!(events[1].tool_use.client_context.is_some(), debug);
            assert_eq!(events[1].tool_use.upstream_context.is_some(), debug);
            if let Some(catalog) = catalog {
                assert_eq!(catalog.discovery_rounds, 1);
                assert_eq!(catalog.loaded_tool_count, 2);
                assert_eq!(catalog.discovery_input_tokens, Some(100));
                assert_eq!(catalog.discovery_latency_ms, Some(events[0].latency_ms));
            }
        }
        let bodies = crate::poison::mutex(&server.state.generation_bodies).clone();
        assert_eq!(
            bodies[0]["metadata"]["task_id"],
            bodies[1]["metadata"]["task_id"]
        );
        assert_eq!(
            bodies[0]["metadata"]["turn_id"],
            bodies[1]["metadata"]["turn_id"]
        );
        assert_eq!(bodies[1]["metadata"]["agent_iteration"], "2");
        // Managed accounts request native SSE even for a buffered client.
        assert_eq!(bodies[0]["stream"], true);
        let lookup_result = bodies[1]["input"].as_array().unwrap().last().unwrap();
        assert_eq!(lookup_result["call_id"], "call_catalog");
        let definitions: Value =
            serde_json::from_str(lookup_result["output"].as_str().unwrap()).unwrap();
        assert_eq!(
            definitions["tools"][0]["parameters"],
            catalog_request(false)["tools"][0]["parameters"]
        );
        *crate::poison::mutex(&server.state.generation_body) = final_response();
        let mut next = request;
        next["stream"] = json!(false);
        let calls = body["output"].as_array().unwrap();
        let mut history = vec![json!({"role":"user","content":"Inspect the synthetic file"})];
        history.extend(calls.clone());
        for call in calls.iter().rev() {
            history.push(json!({"type":"function_call_output","call_id":call["call_id"],"output":"Synthetic file contents"}));
        }
        next["input"] = json!(history);
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&next)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.json::<Value>().await.unwrap()["output"][0]["content"][0]["text"],
            "Ready"
        );
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
        let bodies = crate::poison::mutex(&server.state.generation_bodies);
        let next_input = bodies.last().unwrap()["input"].to_string();
        assert!(next_input.contains("Previously discovered but not called"));
        let events = crate::poison::mutex(&events);
        if debug {
            let context = events
                .last()
                .unwrap()
                .routing
                .as_ref()
                .unwrap()
                .cache_context
                .as_ref()
                .unwrap();
            assert_eq!(
                context.baseline,
                crate::usage::CacheContextBaseline::CompletedRequest
            );
            assert_eq!(context.candidate_changed, Some(false));
            assert_eq!(
                context.upstream_history.input_items,
                Some(bodies.last().unwrap()["input"].as_array().unwrap().len() as u32)
            );
        }
        let catalog = events
            .last()
            .unwrap()
            .tool_use
            .basis_points_catalog
            .as_ref();
        assert_eq!(catalog.is_some(), debug);
        if let Some(catalog) = catalog {
            assert_eq!(catalog.loaded_tool_count, 2);
            assert_eq!(catalog.discovery_rounds, 0);
            assert_eq!(catalog.discovery_latency_ms, None);
        }
        task.abort();
    }
}

#[tokio::test]
async fn mixed_catalog_lookup_returns_parallel_client_calls_without_regeneration() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    server
        .state
        .strict_request_schema
        .store(true, Ordering::SeqCst);
    let mut reply = tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]}));
    for index in 0..3 {
        reply["output"].as_array_mut().unwrap().push(
            tool_response(
                "lookup",
                &format!("call_client_{index}"),
                json!({"path":format!("synthetic-{index}.txt")}),
            )["output"][0]
                .clone(),
        );
    }
    crate::poison::mutex(&server.state.generation_replies).push_back((StatusCode::OK, reply));
    let runtime = Arc::new(runtime(&server).await);
    let (base, task) = recovery::gateway(runtime).await;
    let mut request = catalog_request(false);
    request["parallel_tool_calls"] = json!(true);
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["output"].as_array().unwrap().len(), 3);
    for (index, call) in body["output"].as_array().unwrap().iter().enumerate() {
        assert_eq!(call["name"], "lookup");
        assert_eq!(call["call_id"], format!("call_client_{index}"));
    }
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn incomplete_discovery_answer_keeps_usage_and_never_retries_partial_calls() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let mut incomplete = final_response();
    incomplete["status"] = json!("incomplete");
    incomplete["incomplete_details"] = json!({"reason":"max_output_tokens"});
    incomplete["output"].as_array_mut().unwrap().push(json!({
        "type":"function_call", "name":"run_officejs", "arguments":"{"
    }));
    crate::poison::mutex(&server.state.generation_replies).extend([
        (
            StatusCode::OK,
            tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]})),
        ),
        (StatusCode::OK, incomplete),
    ]);
    let runtime = Arc::new(runtime(&server).await);
    let (base, task) = recovery::gateway(runtime).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&catalog_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["status"], "incomplete");
    assert_eq!(body["incomplete_details"]["reason"], "max_output_tokens");
    assert_eq!(body["usage"]["input_tokens"], 120);
    assert_eq!(body["output"].as_array().unwrap().len(), 1);
    assert_eq!(body["output"][0]["content"][0]["text"], "Ready");
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
    task.abort();
}

#[tokio::test]
async fn parallel_call_diagnostics_separate_native_calls_from_validated_client_calls() {
    for (stream, discovery) in [(false, false), (true, false), (false, true), (true, true)] {
        for parallel in [None, Some(false), Some(true)] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            server
                .state
                .strict_request_schema
                .store(true, Ordering::SeqCst);
            server
                .state
                .stream_generations
                .store(stream, Ordering::SeqCst);
            let mut reply = if discovery {
                tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]}))
            } else {
                tool_response("lookup", "call_client_0", json!({"path":"synthetic-0.txt"}))
            };
            for index in (if discovery { 0 } else { 1 })..2 {
                reply["output"].as_array_mut().unwrap().push(
                    tool_response(
                        "lookup",
                        &format!("call_client_{index}"),
                        json!({"path":format!("synthetic-{index}.txt")}),
                    )["output"][0]
                        .clone(),
                );
            }
            crate::poison::mutex(&server.state.generation_replies)
                .push_back((StatusCode::OK, reply));
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let mut runtime = runtime(&server).await;
            runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let (base, task) = recovery::gateway(Arc::new(runtime)).await;
            let mut request = catalog_request(stream);
            request["relay_bps_tool_catalog"] =
                json!(if discovery { "discovery" } else { "compact" });
            if let Some(parallel) = parallel {
                request["parallel_tool_calls"] = json!(parallel);
            }
            let response = reqwest::Client::new()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let bytes = response.bytes().await.unwrap();
            assert!(!bytes.is_empty());
            let events = crate::poison::mutex(&events);
            let event = events
                .last()
                .expect("completed request records diagnostics");
            assert_eq!(event.tool_use.requested_parallel_tool_calls, parallel);
            assert_eq!(
                event.tool_use.upstream_tool_call_count,
                Some(if discovery { 3 } else { 2 })
            );
            assert_eq!(
                event.tool_use.tool_call_count,
                if parallel == Some(false) { 0 } else { 2 }
            );
            assert_eq!(
                event.tool_use.tool_calls_ready_ms.is_some(),
                parallel != Some(false)
            );
            assert_eq!(event.success, parallel != Some(false));
            assert!(event
                .tool_use
                .upstream_context
                .as_ref()
                .is_some_and(|context| {
                    context.body_bytes > 0 && context.instruction_bytes > 0
                }));
            if parallel == Some(false) {
                assert!(!event.tool_use.text_output);
                assert_eq!(
                    event.tool_use.terminal_output,
                    crate::usage::TerminalOutputKind::Empty
                );
            }
            let transport = event.routing.as_ref().unwrap().transport.as_ref().unwrap();
            assert!(transport.response_headers_ms.is_some());
            assert!(transport.request_preparation_ms.is_some());
            let preparation = transport.request_preparation.as_ref().unwrap();
            let body = preparation.body_ms.unwrap();
            assert!(preparation.schema_validation_ms.unwrap() <= body);
            assert!(
                body + preparation.catalog_ms.unwrap() + preparation.attachments_ms.unwrap()
                    <= transport.request_preparation_ms.unwrap()
            );
            assert_eq!(transport.client_response_ready_ms.is_some(), event.success);
            assert_eq!(
                transport.client_first_output_ms.is_some(),
                stream && event.success
            );
            if !discovery {
                assert!(transport.upstream_response_ms.is_some());
            }
            if let Some(ready) = transport.client_response_ready_ms {
                assert!(ready >= transport.response_headers_ms.unwrap());
                assert!(ready >= event.tool_use.tool_calls_ready_ms.unwrap());
                if let Some(upstream) = transport.upstream_response_ms {
                    assert!(ready >= upstream);
                }
            }
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
            task.abort();
        }
    }
}

#[tokio::test]
async fn catalog_discovery_does_not_replay_failures_or_make_greeting_probes() {
    for failure in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        let runtime = Arc::new(runtime(&server).await);
        let (base, task) = recovery::gateway(runtime).await;
        if failure {
            crate::poison::mutex(&server.state.generation_replies).extend([
                (
                    StatusCode::OK,
                    tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]})),
                ),
                (
                    StatusCode::UNAUTHORIZED,
                    json!({"error":{"message":"synthetic rejection"}}),
                ),
            ]);
        } else {
            *crate::poison::mutex(&server.state.generation_body) = final_response();
        }
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(false))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if failure {
                StatusCode::UNAUTHORIZED
            } else {
                StatusCode::OK
            }
        );
        assert_eq!(
            server.state.generation_requests.load(Ordering::SeqCst),
            if failure { 2 } else { 1 }
        );
        task.abort();
    }
}

#[tokio::test]
async fn catalog_discovery_respects_small_budget_before_first_generation() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    *crate::poison::mutex(&server.state.generation_body) = final_response();
    let mut runtime = runtime(&server).await;
    runtime.diagnostics_enabled = || false;
    runtime.max_retry_candidates.store(1, Ordering::SeqCst);
    let (base, task) = recovery::gateway(Arc::new(runtime)).await;
    let response = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&catalog_request(false))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bodies = crate::poison::mutex(&server.state.generation_bodies);
    assert_eq!(bodies.len(), 1);
    let prompt = bodies[0]["input"][0]["content"][0]["text"]
        .as_str()
        .unwrap();
    assert!(prompt.contains("\"minLength\":1"));
    assert!(!prompt.contains("Relay-local catalog discovery"));
    task.abort();
}

#[tokio::test]
async fn catalog_continuation_rejects_replaced_incarnation_before_dispatch() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = runtime(&server).await;
    let prepared = runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    let key = runtime.authenticate_secret("local-secret").unwrap();
    let route = runtime
        .configured_executor_routes(&key, "gpt-test", &[WireApi::Responses], false)
        .into_iter()
        .find(|route| route.candidate_id == "account-1")
        .unwrap();
    runtime.chatgpt_accounts["account-1"]
        .token_authority
        .register(
            "account-1",
            TokenSet::access_only("synthetic-access", None, 1).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let budget = SharedRequestBudget::for_incoming_request(3);
    let result = runtime
        .send_basis_points_catalog_continuation(
            &route,
            reqwest::Client::new()
                .post(server.responses_url.clone())
                .json(&json!({"model":"gpt-test"})),
            AuthorizationDispatch {
                client_version: None,
                identity_policy: AuthorizationIdentityPolicy::PreserveUpstream,
                turn_scope: None,
                budget: Some(&budget),
                lease: None,
                usage_event: None,
            },
            &prepared.incarnation(),
        )
        .await;
    assert!(matches!(result, Err(AuthorizedRequestError::NotReplayable)));
    assert_eq!(budget.dispatches(), 0);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn automatic_catalog_discovery_uses_existing_chat_messages_and_gemini_bridges() {
    for protocol in [WireApi::ChatCompletions, WireApi::Messages, WireApi::Gemini] {
        for stream in [false, true] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            server
                .state
                .stream_generations
                .store(true, Ordering::SeqCst);
            server
                .state
                .strict_request_schema
                .store(true, Ordering::SeqCst);
            crate::poison::mutex(&server.state.generation_replies).extend([
                (
                    StatusCode::OK,
                    tool_response(
                        "relay.catalog",
                        "call_catalog",
                        json!({"names":["lookup_0"]}),
                    ),
                ),
                (
                    StatusCode::OK,
                    tool_response("lookup_0", "call_client", json!({"path":"synthetic.txt"})),
                ),
            ]);
            let (base, task) = recovery::gateway(Arc::new(runtime(&server).await)).await;
            let description = "Synthetic field contract. ".repeat(60);
            let schema = json!({"type":"object","properties":{"path":{"type":"string","description":description}},"required":["path"]});
            let functions = (0..24).map(|i| json!({"name":format!("lookup_{i}"),"description":"Read a synthetic file.","parameters":schema})).collect::<Vec<_>>();
            let (path, request) = match protocol {
                WireApi::ChatCompletions => (
                    "/v1/chat/completions".to_string(),
                    json!({
                        "model":"gpt-test","stream":stream,"messages":[{"role":"user","content":"Inspect the synthetic file"}],
                        "tools":functions.iter().map(|function| json!({"type":"function","function":function})).collect::<Vec<_>>()
                    }),
                ),
                WireApi::Messages => (
                    "/v1/messages".to_string(),
                    json!({
                        "model":crate::claude_desktop_model_alias("gpt-test"),"stream":stream,"max_tokens":128,
                        "messages":[{"role":"user","content":"Inspect the synthetic file"}],
                        "tools":functions.iter().map(|function| json!({"name":function["name"],"description":function["description"],"input_schema":schema})).collect::<Vec<_>>()
                    }),
                ),
                WireApi::Gemini => (
                    format!(
                        "/v1beta/models/gpt-test:{}",
                        if stream {
                            "streamGenerateContent?alt=sse"
                        } else {
                            "generateContent"
                        }
                    ),
                    json!({
                        "contents":[{"role":"user","parts":[{"text":"Inspect the synthetic file"}]}],
                        "tools":[{"functionDeclarations":functions.iter().map(|function| json!({"name":function["name"],"description":function["description"],"parameters":{
                            "type":"OBJECT","properties":{"path":{"type":"STRING","description":description}},"required":["path"]
                        }})).collect::<Vec<_>>()}]
                    }),
                ),
                _ => unreachable!(),
            };
            let response = reqwest::Client::new()
                .post(format!("{base}{path}"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let body = response.text().await.unwrap();
            assert_eq!(
                status,
                StatusCode::OK,
                "{protocol:?}, stream={stream}: {body}"
            );
            assert!(body.contains("lookup_0"), "{protocol:?}: {body}");
            assert!(!body.contains("relay.catalog"));
            if stream {
                for line in body.lines().filter_map(|line| line.strip_prefix("data: ")) {
                    if let Ok(event) = serde_json::from_str::<Value>(line) {
                        assert_ne!(event["type"], "error", "{event}");
                        assert!(event.get("error").is_none(), "{event}");
                    }
                }
            }
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
            task.abort();
        }
    }
}
