use super::*;

fn visible_message(text: &str) -> Value {
    json!({"id":"msg_visible","type":"message","role":"assistant","status":"completed",
        "content":[{"type":"output_text","text":text,"annotations":[]}]})
}

async fn read_prefix(response: &mut reqwest::Response, text: &str) -> String {
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut prefix = String::new();
        while !prefix.contains(text) {
            let chunk = response
                .chunk()
                .await
                .unwrap()
                .expect("text arrives before the gate opens");
            prefix.push_str(std::str::from_utf8(&chunk).unwrap());
        }
        prefix
    })
    .await
    .expect("first text must not wait for upstream completion")
}

#[tokio::test]
async fn discovery_streams_text_before_completion_with_and_without_a_lookup() {
    for (lookup, reported, json_fallback) in [
        (false, true, false),
        (true, true, false),
        (true, false, false),
        (true, true, true),
    ] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        let gate = Arc::new(Semaphore::new(0));
        crate::poison::mutex(&server.state.stream_gates).push_back(gate.clone());
        let mut first = if lookup {
            tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]}))
        } else {
            final_response()
        };
        first["output"]
            .as_array_mut()
            .unwrap()
            .insert(0, visible_message("Streaming prefix"));
        if !lookup {
            first["output"].as_array_mut().unwrap().truncate(1);
        }
        crate::poison::mutex(&server.state.generation_replies).push_back((StatusCode::OK, first));
        let mut last = tool_response("lookup", "call_client", json!({"path":"synthetic.txt"}));
        if !reported {
            last.as_object_mut().unwrap().remove("usage");
        }
        *crate::poison::mutex(&server.state.generation_body) = last;
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let mut response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(true))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let prefix = read_prefix(&mut response, "Streaming prefix").await;
        assert!(!prefix.contains("response.completed"), "{prefix}");
        assert!(!prefix.contains("run_officejs"), "{prefix}");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        if json_fallback {
            server
                .state
                .stream_generations
                .store(false, Ordering::SeqCst);
        }
        gate.add_permits(1);
        let body = prefix + &response.text().await.unwrap();
        assert!(!body.contains("relay.catalog"), "{body}");
        let mut pending = body.as_bytes().to_vec();
        let mut completed = Vec::new();
        let mut text = String::new();
        while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
            let event: Value =
                serde_json::from_slice(&crate::protocol::sse_data(&frame)).unwrap_or(Value::Null);
            if event["type"] == "response.completed" {
                completed.push(event["response"].clone());
            }
            if event["type"] == "response.output_text.delta" {
                text.push_str(event["delta"].as_str().unwrap());
            }
            assert_ne!(event["type"], "error", "{event}");
        }
        assert_eq!(text, "Streaming prefix");
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0]["output"][0]["content"][0]["text"], text);
        let expected = if lookup { 2 } else { 1 };
        assert_eq!(
            server.state.generation_requests.load(Ordering::SeqCst),
            expected
        );
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), expected);
        let final_event = events.last().unwrap();
        assert!(final_event.success, "{final_event:?}");
        assert!(final_event.ttft_ms.is_some());
        assert_eq!(final_event.attempt as usize, expected);
        if !reported {
            assert_eq!(final_event.input_tokens, None);
            assert_eq!(final_event.output_tokens, None);
        } else if lookup {
            assert_eq!(final_event.input_tokens, Some(100));
        }
        if lookup {
            assert_eq!(completed[0]["output"][1]["name"], "lookup");
        }
        task.abort();
    }
}

#[tokio::test]
async fn discovery_json_diagnostics_survive_buffered_and_streamed_errors() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(stream, Ordering::SeqCst);
        let mut reply = tool_response("lookup", "call_invalid_json", json!({"path":"synthetic"}));
        reply["output"][0]["arguments"] = json!({
            "references":["lookup"],"code":r#"{"path":"synthetic-private\u12X4"}"#
        })
        .to_string()
        .into();
        *crate::poison::mutex(&server.state.generation_body) = reply;
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| {
            crate::poison::mutex(&captured).push(event);
        });
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(stream))
            .send()
            .await
            .unwrap();
        let body = response.text().await.unwrap();
        assert!(body.contains("json_reason=invalid_escape"), "{body}");
        assert!(body.contains("payload_shape=json_object"));
        assert!(!body.contains("synthetic-private"));
        assert!(!body.contains("response.completed"));
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        let events = crate::poison::mutex(&events);
        let diagnostic = events.last().unwrap().upstream_error.as_ref().unwrap();
        let message = diagnostic.message.as_ref().unwrap();
        assert!(message.contains("json_reason=invalid_escape"), "{message}");
        assert!(!message.contains("synthetic-private"));
        task.abort();
    }
}

#[tokio::test]
async fn discovery_never_releases_invalid_tool_arguments_after_streamed_text() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    server
        .state
        .stream_generations
        .store(true, Ordering::SeqCst);
    let gate = Arc::new(Semaphore::new(0));
    crate::poison::mutex(&server.state.stream_gates).push_back(gate.clone());
    let mut reply = tool_response("lookup", "call_invalid", json!({"path":""}));
    reply["output"]
        .as_array_mut()
        .unwrap()
        .insert(0, visible_message("Checking the file"));
    *crate::poison::mutex(&server.state.generation_body) = reply;
    let (base, task) = recovery::gateway(Arc::new(runtime(&server).await)).await;
    let mut response = reqwest::Client::new()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&catalog_request(true))
        .send()
        .await
        .unwrap();
    let prefix = read_prefix(&mut response, "Checking the file").await;
    assert!(!prefix.contains("\"call_id\":\"call_invalid\""));
    assert!(!prefix.contains("\"type\":\"function_call\""));
    gate.add_permits(1);
    let body = prefix + &response.text().await.unwrap();
    assert!(body.contains("event: error"), "{body}");
    assert!(!body.contains("\"call_id\":\"call_invalid\""), "{body}");
    assert!(!body.contains("response.completed"), "{body}");
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
    task.abort();
}

#[tokio::test]
async fn cancelling_discovery_drops_the_upstream_without_starting_another_generation() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    server
        .state
        .stream_generations
        .store(true, Ordering::SeqCst);
    let gate = Arc::new(Semaphore::new(0));
    crate::poison::mutex(&server.state.stream_gates).push_back(gate.clone());
    let mut reply = tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]}));
    reply["output"]
        .as_array_mut()
        .unwrap()
        .insert(0, visible_message("Before cancellation"));
    *crate::poison::mutex(&server.state.generation_body) = reply;
    let finished = Arc::new(Notify::new());
    let notified = finished.clone();
    let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
    let captured = events.clone();
    let mut runtime = runtime(&server).await;
    runtime.usage = Arc::new(move |event| {
        crate::poison::mutex(&captured).push(event);
        notified.notify_one();
    });
    let (base, task) = recovery::gateway(Arc::new(runtime)).await;
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let mut response = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&catalog_request(true))
        .send()
        .await
        .unwrap();
    read_prefix(&mut response, "Before cancellation").await;
    drop(response);
    drop(client);
    tokio::time::timeout(std::time::Duration::from_secs(3), finished.notified())
        .await
        .expect("cancellation settles the live stream without waiting for upstream completion");
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
    assert!(!crate::poison::mutex(&events).last().unwrap().success);
    gate.add_permits(1);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
    task.abort();
}
