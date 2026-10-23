use super::*;

#[tokio::test]
async fn repeated_lookup_reuses_schema_without_replaying_client_tools_or_history() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        crate::poison::mutex(&server.state.generation_replies).extend([
            (
                StatusCode::OK,
                tool_response("relay.catalog", "call_first", json!({"names":["lookup"]})),
            ),
            (
                StatusCode::OK,
                tool_response(
                    "relay.catalog",
                    "call_repeat",
                    json!({"query":"lookup","limit":1}),
                ),
            ),
            (
                StatusCode::OK,
                tool_response("lookup", "call_client", json!({"path":"synthetic.txt"})),
            ),
        ]);
        let mut request = catalog_request(stream);
        request["tools"][0]["description"] = json!("Synthetic schema deduplication marker");
        request["input"] = json!("Synthetic conversation deduplication marker");
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let reply = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), StatusCode::OK);
        let body = reply.text().await.unwrap();
        assert!(body.contains("call_client"), "{body}");
        assert!(!body.contains("call_repeat"));
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
        let bodies = crate::poison::mutex(&server.state.generation_bodies);
        for (index, body) in bodies.iter().enumerate() {
            let text = body.to_string();
            assert_eq!(
                text.matches("Synthetic conversation deduplication marker")
                    .count(),
                1
            );
            assert_eq!(
                text.matches("Synthetic schema deduplication marker")
                    .count(),
                usize::from(index != 0)
            );
        }
        let repeated = bodies[2]["input"].as_array().unwrap().last().unwrap();
        assert_eq!(repeated["call_id"], "call_repeat");
        let reused: Value = serde_json::from_str(repeated["output"].as_str().unwrap()).unwrap();
        assert_eq!(reused["tools"], json!([]));
        assert_eq!(
            reused["reused"],
            json!([{"name":"lookup","call_id":"call_first"}])
        );
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 3);
        assert!(events[2].success);
        let first_catalog = events[0].tool_use.basis_points_catalog.as_ref().unwrap();
        let last_catalog = events[2].tool_use.basis_points_catalog.as_ref().unwrap();
        assert_eq!(
            last_catalog.rendered_schema_bytes,
            first_catalog.rendered_schema_bytes
        );
        assert_eq!(last_catalog.discovery_rounds, 2);
        assert_eq!(last_catalog.discovery_input_tokens, Some(200));
        task.abort();
    }
}

#[tokio::test]
async fn last_completed_lookup_survives_exhausted_dispatch_budget() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        crate::poison::mutex(&server.state.generation_replies).extend([
            (
                StatusCode::OK,
                tool_response("relay.catalog", "call_first", json!({"names":["first"]})),
            ),
            (
                StatusCode::OK,
                tool_response("relay.catalog", "call_last", json!({"names":["lookup"]})),
            ),
            (
                StatusCode::OK,
                tool_response("lookup", "call_answer", json!({"path":"synthetic.txt"})),
            ),
        ]);
        let mut request = catalog_request(stream);
        request["tools"][0]["description"] = json!("Last completed lookup contract");
        request["tools"].as_array_mut().unwrap().extend([
            json!({"type":"function","name":"first","description":"First completed lookup contract","parameters":{"type":"object","properties":{}}}),
            json!({"type":"function","name":"previous","parameters":{"type":"object","properties":{}}}),
        ]);
        request["input"] = json!([
            {"type":"function_call","name":"previous","call_id":"call_prior","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_prior","output":"done"},
            {"role":"user","content":"Synthetic continuation"}
        ]);
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.max_retry_candidates.store(2, Ordering::SeqCst);
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let client = reqwest::Client::new();
        let first = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(first.contains("output.relay.catalog.budget"), "{first}");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);

        let retry = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(retry.status(), StatusCode::OK);
        let body = retry.text().await.unwrap();
        assert!(body.contains("call_answer"), "{body}");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
        let bodies = crate::poison::mutex(&server.state.generation_bodies);
        let retry_body = bodies[2].to_string();
        assert!(retry_body.contains("First completed lookup contract"));
        assert!(
            retry_body.contains("Last completed lookup contract"),
            "stream={stream}: the last lookup must survive budget exhaustion"
        );
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 3, "one usage event per physical generation");
        assert_eq!(events[1].attempt, 2);
        assert!(!events[1].success);
        let catalog = events[1].tool_use.basis_points_catalog.as_ref().unwrap();
        assert_eq!(catalog.discovery_rounds, 2);
        assert_eq!(catalog.discovery_input_tokens, Some(200));
        task.abort();
    }
}

#[tokio::test]
async fn completed_lookup_is_preloaded_on_explicit_retry_after_failed_answer() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        crate::poison::mutex(&server.state.generation_replies).extend([
            (
                StatusCode::OK,
                tool_response(
                    "relay.catalog",
                    "call_catalog",
                    json!({"names":["lookup", "retired"]}),
                ),
            ),
            (
                StatusCode::BAD_REQUEST,
                json!({"error":{"code":"invalid_prompt","message":"Synthetic rejection"}}),
            ),
            (
                StatusCode::OK,
                tool_response("lookup", "call_answer", json!({"path":"synthetic.txt"})),
            ),
        ]);
        let mut request = catalog_request(stream);
        request["tools"][0]["description"] = json!("Synthetic lookup contract must be preloaded");
        request["tools"].as_array_mut().unwrap().push(json!({
            "type":"function","name":"previous","parameters":{"type":"object","properties":{}}
        }));
        request["tools"].as_array_mut().unwrap().push(json!({
            "type":"function","name":"retired","description":"Removed contract must not return","parameters":{"type":"object","properties":{}}
        }));
        request["input"] = json!([
            {"type":"function_call","name":"previous","call_id":"call_prior","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_prior","output":"done"},
            {"role":"user","content":format!("synthetic-history-marker {}", "Synthetic context. ".repeat(8192))}
        ]);
        let (base, task) = recovery::gateway(Arc::new(runtime(&server).await)).await;
        let client = reqwest::Client::new();
        let first = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(
            first.status(),
            if stream {
                StatusCode::OK
            } else {
                StatusCode::BAD_REQUEST
            }
        );
        let first = first.text().await.unwrap();
        assert!(!first.contains("response.completed"));
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);

        // This is an explicit client retry, never an automatic generation replay.
        request["tools"][0]["description"] = json!("Current lookup contract must be preloaded");
        request["tools"][0]["parameters"]["properties"]["path"]["maxLength"] = json!(64);
        request["tools"]
            .as_array_mut()
            .unwrap()
            .retain(|tool| tool["name"] != "retired");
        let retry = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&request)
            .send()
            .await
            .unwrap();
        let status = retry.status();
        let body = retry.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("lookup"), "{body}");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
        let bodies = crate::poison::mutex(&server.state.generation_bodies);
        assert_eq!(bodies.len(), 3);
        assert!(!bodies[0]
            .to_string()
            .contains("Synthetic lookup contract must be preloaded"));
        let retry_body = bodies[2].to_string();
        assert!(retry_body.contains("Current lookup contract must be preloaded"));
        assert!(retry_body.contains("maxLength"));
        assert!(!retry_body.contains("Synthetic lookup contract must be preloaded"));
        assert!(!retry_body.contains("retired"));
        assert!(!retry_body.contains("Removed contract must not return"));
        for body in bodies.iter() {
            assert_eq!(
                body.to_string().matches("synthetic-history-marker").count(),
                1
            );
        }
        task.abort();
    }
}

#[tokio::test]
async fn invalid_local_lookup_can_be_corrected_without_rotating_or_replaying_client_tools() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        crate::poison::mutex(&server.state.generation_replies).extend([
            (
                StatusCode::OK,
                tool_response(
                    "relay.catalog",
                    "call_bad_lookup",
                    json!({"names":["unknown"]}),
                ),
            ),
            (
                StatusCode::OK,
                tool_response(
                    "relay.catalog",
                    "call_good_lookup",
                    json!({"names":["lookup"]}),
                ),
            ),
            (
                StatusCode::OK,
                tool_response("lookup", "call_client", json!({"path":"synthetic.txt"})),
            ),
        ]);
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(stream))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.bytes().await.unwrap();
        let body: Value = if stream {
            let mut pending = bytes.to_vec();
            let mut completed = None;
            while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
                let event: Value =
                    serde_json::from_slice(&crate::protocol::sse_data(&frame)).unwrap();
                assert_ne!(event["type"], "error");
                if event["type"] == "response.completed" {
                    assert!(completed.is_none());
                    completed = Some(event["response"].clone());
                }
            }
            completed.unwrap()
        } else {
            serde_json::from_slice(&bytes).unwrap()
        };
        assert_eq!(body["output"].as_array().unwrap().len(), 1);
        assert_eq!(body["output"][0]["name"], "lookup");
        assert_eq!(body["output"][0]["call_id"], "call_client");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 3);
        for event in events.iter() {
            assert_eq!(event.request_id, events[0].request_id);
            assert_eq!(event.candidate_id, events[0].candidate_id);
            assert_eq!(event.input_tokens, Some(100));
            assert_eq!(event.output_tokens, Some(10));
        }
        let last = events.last().unwrap();
        assert!(last.success);
        assert_eq!(
            last.tool_use
                .basis_points_catalog
                .as_ref()
                .unwrap()
                .discovery_rounds,
            2
        );
        task.abort();
    }
}
