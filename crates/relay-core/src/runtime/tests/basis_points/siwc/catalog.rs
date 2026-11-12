use super::*;
use futures_util::{SinkExt, StreamExt};
use reqwest_websocket::{Message, Upgrade};

mod full_history;
mod lifecycle;

fn request(stream: bool) -> Value {
    let mut tools = (0..32).map(|i| json!({"type":"function","name":format!("lookup_{i}"),
        "description":"Synthetic searchable catalog description. ".repeat(40),
        "parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}
    })).collect::<Vec<_>>();
    tools.push(json!({"type":"namespace","name":"files","description":"Synthetic namespace usage rules",
        "tools":[{"type":"function","name":"read","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}]}));
    tools.push(json!({"type":"custom","name":"edit","format":{"type":"text"}}));
    json!({"model":"gpt-test","stream":stream,"input":[{"role":"user","content":"Synthetic request"}],
        "tools":tools,"parallel_tool_calls":true,"reasoning":{"effort":"high"},
        "prompt_cache_key":"synthetic-native-catalog","prompt_cache_options":{"mode":"implicit","ttl":"30m"}})
}

fn response(id: &str, output: Value) -> Value {
    json!({"id":id,"object":"response","model":"gpt-test","status":"completed","output":output,
        "usage":{"input_tokens":100,"output_tokens":10,"total_tokens":110,"input_tokens_details":{"cached_tokens":64}}})
}

fn lookup(id: &str) -> Value {
    response(
        id,
        json!([{"type":"function_call","id":format!("fc_{id}"),"call_id":format!("call_{id}"),
        "name":"relay_catalog","arguments":json!({"names":["files.read","edit"]}).to_string(),"status":"completed"}]),
    )
}

fn calls() -> Value {
    response(
        "resp_native_calls",
        json!([
            {"type":"function_call","id":"fc_native_read","call_id":"call_native_read","namespace":"files","name":"read","arguments":"{\"path\":\"synthetic.txt\"}","status":"completed"},
            {"type":"custom_tool_call","id":"ct_native_edit","call_id":"call_native_edit","name":"edit","input":"Synthetic raw\n\"patch\" \\text","status":"completed"}
        ]),
    )
}

fn answer() -> Value {
    response(
        "resp_native_answer",
        json!([{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Synthetic ready"}]}]),
    )
}

fn terminal(text: &str, stream: bool) -> Value {
    assert!(
        !text.contains("relay_catalog"),
        "local lookup leaked: {text}"
    );
    if !stream {
        return serde_json::from_str(text).unwrap();
    }
    let events = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.strip_prefix("data: ")?).ok())
        .collect::<Vec<_>>();
    assert!(
        !events.iter().any(|event| event["type"] == "error"),
        "{text}"
    );
    for pair in events.windows(2) {
        if let (Some(a), Some(b)) = (
            pair[0]["sequence_number"].as_u64(),
            pair[1]["sequence_number"].as_u64(),
        ) {
            assert!(a < b);
        }
    }
    events
        .into_iter()
        .find(|event| event["type"] == "response.completed")
        .expect("terminal")["response"]
        .clone()
}

#[tokio::test]
async fn siwc_native_catalog_loads_only_requested_tools_and_preserves_roundtrip() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        let mut first = lookup("resp_native_lookup");
        first["output"].as_array_mut().unwrap().insert(0, json!({"type":"reasoning","id":"rs_synthetic","encrypted_content":"synthetic-opaque-reasoning","summary":[]}));
        crate::poison::mutex(&siwc.state.stream_bodies)
            .push_back(super::reasoning::reasoning_stream(&first));
        crate::poison::mutex(&siwc.state.generation_replies)
            .extend([(StatusCode::OK, first), (StatusCode::OK, calls())]);
        *crate::poison::mutex(&siwc.state.generation_body) = answer();
        let runtime = runtime(&siwc, &native).await;
        let (base, task) = recovery::gateway(runtime).await;
        let client = retry_admission::bounded_client();
        let mut payload = request(stream);
        payload["max_output_tokens"] = json!(1024);
        let result = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&payload)
            .send()
            .await
            .unwrap();
        let status = result.status();
        let text = result.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{text}");
        let result = terminal(&text, stream);
        assert_eq!(
            result["output"][0]["encrypted_content"],
            "synthetic-opaque-reasoning"
        );
        assert_eq!(result["output"][1], calls()["output"][0]);
        assert_eq!(result["output"][2], calls()["output"][1]);
        assert_eq!(result["usage"]["input_tokens_details"]["cached_tokens"], 64);
        payload["previous_response_id"] = result["id"].clone();
        payload["input"] = json!([
            {"type":"function_call_output","call_id":"call_native_read","output":"Synthetic read result"},
            {"type":"custom_tool_call_output","call_id":"call_native_edit","output":"Synthetic edit result"}
        ]);
        let result = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&payload)
            .send()
            .await
            .unwrap();
        let status = result.status();
        let text = result.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{text}");
        assert_eq!(
            terminal(&text, stream)["output"][0]["content"][0]["text"],
            "Synthetic ready"
        );
        let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
        assert_eq!(bodies.len(), 3);
        for body in bodies.iter() {
            assert!(body.get("max_output_tokens").is_none());
            assert!(body.get("metadata").is_none());
            assert!(body.get("model_selection").is_none());
            assert!(body.get("previous_response_id").is_none());
            assert_eq!(body["reasoning"], json!({"effort":"high"}));
            assert_eq!(
                body["prompt_cache_options"],
                json!({"mode":"implicit","ttl":"30m"})
            );
            assert_eq!(body["prompt_cache_key"], "synthetic-native-catalog");
        }
        let stats = |body: &Value| crate::tool_policy::catalog_stats_with_sizes(body, true);
        assert_eq!(stats(&bodies[0]).count, 1);
        assert_eq!(stats(&bodies[1]).count, 3);
        assert!(stats(&bodies[0]).bytes < stats(&request(stream)).bytes / 10);
        let tail = bodies[1]["input"].as_array().unwrap().last().unwrap();
        let namespace = tail["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["type"] == "namespace")
            .unwrap();
        assert_eq!(namespace["description"], "Synthetic namespace usage rules");
        assert_eq!(namespace["tools"][0]["name"], "read");
        assert!(
            bodies[1]["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["encrypted_content"] == "synthetic-opaque-reasoning")
        );
        assert!(
            bodies[2]["input"]
                .as_array()
                .unwrap()
                .iter()
                .any(|item| item["call_id"] == "call_native_edit"
                    && item["type"] == "custom_tool_call_output")
        );
        assert_eq!(siwc.state.attachment_requests.load(Ordering::SeqCst), 0);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_native_catalog_websocket_hides_lookup_and_keeps_client_calls() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    siwc.state.stream_generations.store(true, Ordering::SeqCst);
    crate::poison::mutex(&siwc.state.generation_replies).extend([
        (StatusCode::OK, lookup("resp_ws_lookup")),
        (StatusCode::OK, calls()),
    ]);
    let runtime = runtime(&siwc, &native).await;
    let (base, task) = recovery::gateway(runtime).await;
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
    let mut payload = request(true);
    payload["type"] = json!("response.create");
    payload["max_output_tokens"] = json!(1024);
    socket
        .send(Message::Text(payload.to_string()))
        .await
        .unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(message) = socket.next().await {
            if let Message::Text(text) = message.unwrap() {
                assert!(!text.contains("relay_catalog"));
                let event: Value = serde_json::from_str(&text).unwrap();
                assert_ne!(event["type"], "error", "{event}");
                if event["type"] == "response.completed" {
                    return event["response"].clone();
                }
            }
        }
        panic!("No terminal response");
    })
    .await
    .unwrap();
    assert_eq!(result["output"], calls()["output"]);
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 2);
    assert!(
        crate::poison::mutex(&siwc.state.generation_bodies)
            .iter()
            .all(|body| body.get("max_output_tokens").is_none())
    );
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn siwc_native_catalog_rejects_invalid_calls_without_retrying_execution() {
    for stream in [false, true] {
        for invalid in ["schema", "unknown", "parallel"] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            siwc.state.stream_generations.store(true, Ordering::SeqCst);
            let mut call = calls();
            match invalid {
                "schema" => call["output"][0]["arguments"] = json!("{}"),
                "unknown" => call["output"][0]["name"] = json!("not_declared"),
                _ => {}
            }
            crate::poison::mutex(&siwc.state.generation_replies).extend([
                (StatusCode::OK, lookup("resp_invalid_lookup")),
                (StatusCode::OK, call),
            ]);
            let (base, task) = recovery::gateway(runtime(&siwc, &native).await).await;
            let mut payload = request(stream);
            if invalid == "parallel" {
                payload["parallel_tool_calls"] = json!(false);
            }
            let result = retry_admission::bounded_client()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&payload)
                .send()
                .await
                .unwrap();
            let text = result.text().await.unwrap();
            assert!(text.contains("error"), "{invalid}: {text}");
            assert!(
                !text.contains("call_native_read"),
                "executable call leaked: {text}"
            );
            assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 2);
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_native_catalog_reports_actual_per_generation_usage_and_loaded_counts() {
    for (stream, debug, upstream_stream) in [
        (false, false, true),
        (true, false, true),
        (false, true, true),
        (true, true, true),
        (false, true, false),
        (true, true, false),
    ] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state
            .stream_generations
            .store(upstream_stream, Ordering::SeqCst);
        let mut lookup_response = lookup("resp_usage_lookup");
        let mut final_response = calls();
        for (index, response) in [&mut lookup_response, &mut final_response]
            .into_iter()
            .enumerate()
        {
            response["service_tier"] = json!(if index == 0 { "default" } else { "priority" });
            response["usage"]["input_tokens_details"]["cache_write_tokens"] = json!(20);
            response["usage"]["input_tokens_details"]["cache_write_tokens_details"] = json!({
                "ttl_30m_tokens":if index == 0 { 20 } else { 0 },
                "ttl_12h_tokens":if index == 0 { 0 } else { 20 }
            });
        }
        crate::poison::mutex(&siwc.state.generation_replies).extend([
            (StatusCode::OK, lookup_response),
            (StatusCode::OK, final_response),
        ]);
        let mut runtime = runtime(&siwc, &native).await;
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        Arc::get_mut(&mut runtime).unwrap().diagnostics_enabled =
            if debug { || true } else { || false };
        Arc::get_mut(&mut runtime).unwrap().usage =
            Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(runtime).await;
        let result = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request(stream))
            .send()
            .await
            .unwrap();
        let text = result.text().await.unwrap();
        terminal(&text, stream);
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 2);
        assert!(!events[0].success);
        assert_eq!(
            events[0].routing.as_ref().unwrap().attempt_outcome,
            Some(crate::AttemptOutcome::ToolDiscovery)
        );
        assert!(events[1].success);
        assert_eq!(events[0].tool_use.forwarded_tool_count, 1);
        assert_eq!(events[1].tool_use.forwarded_tool_count, 3);
        for (index, event) in events.iter().enumerate() {
            let (short, long, ttl) = if index == 0 {
                (20, 0, "30m")
            } else {
                (0, 20, "12h")
            };
            assert_eq!(event.input_tokens, Some(100));
            assert_eq!(event.output_tokens, Some(10));
            assert_eq!(event.cached_input_tokens, Some(64));
            assert_eq!(event.cache_write_input_tokens, Some(20));
            assert_eq!(event.total_tokens, Some(110));
            assert_eq!(
                event.applied_service_tier.as_deref(),
                Some(if index == 0 { "default" } else { "priority" })
            );
            assert_eq!(event.cache_write_ttl.as_deref(), Some(ttl));
            let observed = event.tool_use.provider_usage.as_ref().unwrap();
            assert_eq!(observed.cache_write_tokens_by_ttl.get("30m"), Some(&short));
            assert_eq!(observed.cache_write_tokens_by_ttl.get("12h"), Some(&long));
            if upstream_stream {
                let first = event.ttft_ms.unwrap_or_else(|| {
                    panic!(
                        "missing upstream timing: stream={stream}, debug={debug}, attempt={}",
                        event.attempt
                    )
                });
                assert!(first <= event.latency_ms);
                assert_eq!(event.generation_ms, Some(event.latency_ms - first));
            } else {
                assert_eq!(event.ttft_ms, None, "JSON has no first-token evidence");
                assert_eq!(event.generation_ms, None);
            }
        }
        assert_eq!(events[1].tool_use.tool_call_count, 2);
        let catalog = events[1].tool_use.basis_points_catalog.as_ref();
        assert_eq!(catalog.is_some(), debug);
        if let Some(catalog) = catalog {
            assert_eq!(catalog.loaded_tool_count, 2);
            assert_eq!(catalog.discovery_latency_ms, Some(events[0].latency_ms));
        }
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 2);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_buffered_catalog_keeps_timing_without_lookup_and_after_upstream_failure() {
    for completed in [true, false] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        let mut terminal_response = answer();
        let kind = if completed {
            "response.completed"
        } else {
            terminal_response["status"] = json!("failed");
            terminal_response["error"] = json!({
                "code":"synthetic_failure","message":"Synthetic failure after output"
            });
            "response.failed"
        };
        let terminal_event = json!({"type":kind,"response":terminal_response});
        crate::poison::mutex(&siwc.state.stream_bodies).push_back(
            format!(
                "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"Synthetic ready\"}}\n\ndata: {terminal_event}\n\n"
            )
            .into_bytes(),
        );
        let mut runtime = runtime(&siwc, &native).await;
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        Arc::get_mut(&mut runtime).unwrap().diagnostics_enabled = || true;
        Arc::get_mut(&mut runtime).unwrap().usage =
            Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(runtime).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request(false))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().is_success(), completed);
        let _ = response.bytes().await.unwrap();
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.success, completed);
        let first = event
            .ttft_ms
            .expect("actual delta survives catalog buffering");
        assert!(first <= event.latency_ms);
        assert_eq!(event.generation_ms, Some(event.latency_ms - first));
        assert_eq!(event.input_tokens, Some(100));
        assert_eq!(event.cached_input_tokens, Some(64));
        assert_eq!(event.output_tokens, Some(10));
        assert_eq!(
            event
                .tool_use
                .basis_points_catalog
                .as_ref()
                .unwrap()
                .discovery_rounds,
            0
        );
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}
