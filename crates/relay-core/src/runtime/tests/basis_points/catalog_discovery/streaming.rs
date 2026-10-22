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
async fn discovery_rejects_conflicting_or_unfinished_parallel_tool_identity() {
    for mode in ["single", "batch", "catalog"] {
        for invalid in ["changed", "orphan", "delta"] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            server
                .state
                .stream_generations
                .store(true, Ordering::SeqCst);
            let reply = match mode {
                "batch" => super::batch::batch_response(),
                "catalog" => {
                    tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]}))
                }
                _ => tool_response("lookup", "call_client", json!({"path":"synthetic"})),
            };
            let mut item = reply["output"][0].clone();
            item["call_id"] = json!("call_unfinished");
            if invalid != "changed" {
                item["id"] = json!("fc_unfinished");
            }
            let prefix = if invalid == "delta" {
                json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":item["id"],"delta":"{"})
            } else {
                json!({"type":"response.output_item.added","output_index":0,"item":item})
            };
            crate::poison::mutex(&server.state.stream_prefixes)
                .push_back(format!("data: {prefix}\n\n").into_bytes());
            *crate::poison::mutex(&server.state.generation_body) = reply;
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let mut runtime = runtime(&server).await;
            runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let (base, task) = recovery::gateway(Arc::new(runtime)).await;
            let body = reqwest::Client::new()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&catalog_request(true))
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap();
            assert!(body.contains("stream.tool."), "{mode}/{invalid}: {body}");
            assert!(!body.contains("event: response.completed"));
            assert!(!body.contains("event: response.function_call_arguments.delta"));
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
            let events = crate::poison::mutex(&events);
            assert_eq!(events.len(), 1);
            assert!(!events[0].success);
            assert_eq!(events[0].tool_use.tool_call_count, 0);
            assert!(events[0].tool_use.tool_calls_ready_ms.is_none());
            task.abort();
        }
    }
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
async fn discovery_keeps_hosted_search_in_the_final_messages_tool_turn() {
    use crate::{
        AdapterRequestContext, CacheWriteTtl, MessagesReasoningMode, SourceAdapter, WireApi,
    };

    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        let mut first = tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]}));
        first["output"].as_array_mut().unwrap().splice(
            0..0,
            [
                json!({"type":"web_search_call","id":"ws_synthetic","status":"completed"}),
                visible_message("Checking the synthetic path"),
            ],
        );
        crate::poison::mutex(&server.state.generation_replies).push_back((StatusCode::OK, first));
        *crate::poison::mutex(&server.state.generation_body) =
            tool_response("lookup", "call_client", json!({"path":"synthetic.txt"}));
        let (base, task) = recovery::gateway(Arc::new(runtime(&server).await)).await;
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(stream))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.bytes().await.unwrap();
        let request = json!({"max_tokens":4096,"messages":[{"role":"user","content":"Inspect the synthetic file"}],
            "tools":[{"name":"lookup","input_schema":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}}]});
        let prepared = SourceAdapter::MessagesToResponses
            .prepare_request(AdapterRequestContext {
                client_wire_api: WireApi::Messages,
                request: &request,
                model: "gpt-test",
                stream,
                reasoning_mode: MessagesReasoningMode::Adaptive,
                cache_write_ttl: CacheWriteTtl::Provider,
                previous: None,
                response_scope: "synthetic-discovery",
                response_id_seed: "synthetic-search",
            })
            .unwrap();
        let translated = if stream {
            let mut bridge = prepared.into_stream_bridge().unwrap();
            for chunk in bytes.chunks(7) {
                bridge.push(chunk);
            }
            bridge.finish();
            bridge
                .completed()
                .expect("discovery must preserve the complete output sequence")
                .response_body
                .clone()
        } else {
            assert_eq!(
                serde_json::from_slice::<Value>(&bytes).unwrap()["output"][0]["type"],
                "web_search_call"
            );
            prepared
                .translate_response_bytes(&bytes)
                .unwrap()
                .unwrap()
                .translated_response()
                .response_body
                .clone()
        };
        assert_eq!(translated["stop_reason"], "tool_use");
        assert_eq!(
            translated["content"][0]["text"],
            "Checking the synthetic path"
        );
        assert_eq!(translated["content"][1]["name"], "lookup");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
        task.abort();
    }
}

#[tokio::test]
async fn discovery_preserves_rate_limits_after_output_without_replaying() {
    for (stream, untyped) in [(true, false), (true, true), (false, false), (false, true)] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        let gate = Arc::new(Semaphore::new(usize::from(!stream)));
        crate::poison::mutex(&server.state.stream_gates).push_back(gate.clone());
        let error = json!({
            "code":"rate_limit_exceeded","type":"tokens",
            "message":"synthetic-private-provider-detail","retry_after":120
        });
        let usage = json!({"input_tokens":17,"output_tokens":3,"total_tokens":20});
        let terminal = if untyped {
            json!({"error":error,"usage":usage})
        } else {
            json!({"type":"response.failed","response":{
                "id":"resp_limited","status":"failed","output":[],
                "error":error,"usage":usage
            }})
        };
        let prefix = [
            json!({"type":"response.created","response":{"id":"resp_limited","status":"in_progress","output":[]}}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"Before the limit"}),
            terminal,
        ]
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
        crate::poison::mutex(&server.state.stream_prefixes).push_back(prefix.into_bytes());
        // A later terminal must not replace the first failure or release more output.
        *crate::poison::mutex(&server.state.generation_body) = final_response();
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let finished = Arc::new(Notify::new());
        let notified = finished.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| {
            crate::poison::mutex(&captured).push(event);
            notified.notify_one();
        });
        let runtime = Arc::new(runtime);
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let before = runtime_now_ms();
        let mut response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(stream))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if stream {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
        let prefix = if stream {
            let prefix = read_prefix(&mut response, "Before the limit").await;
            gate.add_permits(1);
            prefix
        } else {
            String::new()
        };
        let body = prefix + &response.text().await.unwrap();
        assert!(body.contains("rate limit was reached"), "{body}");
        assert!(!body.contains("Basis Points stream could not be completed safely"));
        assert!(!body.contains("synthetic-private-provider-detail"));
        assert!(!body.contains("response.completed"));
        assert!(!body.contains("Ready"));
        tokio::time::timeout(std::time::Duration::from_secs(3), finished.notified())
            .await
            .unwrap();
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        let retry_at = runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .cooldowns["gpt-test"];
        assert!(
            retry_at >= before + 120_000,
            "provider pause was shortened: stream={stream}, untyped={untyped}"
        );
        assert!(retry_at <= runtime_now_ms() + 120_000);
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert!(!event.success);
        assert_eq!(event.http_status, 429);
        assert_eq!(
            event.error_category.as_deref(),
            Some(crate::error_codes::UPSTREAM_RATE_LIMITED)
        );
        assert_eq!(event.input_tokens, Some(17));
        assert_eq!(event.output_tokens, Some(3));
        let details = event.upstream_error.as_ref().unwrap();
        assert_eq!(details.http_status, Some(200));
        assert_eq!(details.code.as_deref(), Some("rate_limit_exceeded"));
        assert_eq!(details.error_type.as_deref(), Some("tokens"));
        assert_eq!(
            details.message.as_deref(),
            Some("synthetic-private-provider-detail")
        );
        task.abort();
    }
}

#[tokio::test]
async fn discovery_continuation_retains_redacted_provider_error_without_replay() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        crate::poison::mutex(&server.state.generation_replies).extend([
            (
                StatusCode::OK,
                tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]})),
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                json!({"error":{
                    "code":"rate_limit_exceeded","type":"tokens",
                    "retry_after":120,
                    "request_id":"req_synthetic_discovery_limit",
                    "message":"TPM limit reached; contact synthetic-private@example.invalid at https://example.invalid/private"
                }}),
            ),
        ]);
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let finished = Arc::new(Notify::new());
        let notified = finished.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| {
            crate::poison::mutex(&captured).push(event);
            notified.notify_one();
        });
        let runtime = Arc::new(runtime);
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let before = runtime_now_ms();
        let response = reqwest::Client::new()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&catalog_request(stream))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if stream {
                StatusCode::OK
            } else {
                StatusCode::TOO_MANY_REQUESTS
            }
        );
        let body = response.text().await.unwrap();
        assert!(body.contains("rate limit was reached"), "{body}");
        assert!(!body.contains("synthetic-private"));
        assert!(!body.contains("response.completed"));
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                let notified = finished.notified();
                if crate::poison::mutex(&events).len() == 2 {
                    break;
                }
                notified.await;
            }
        })
        .await
        .unwrap();
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
        let retry_at = runtime
            .lock_scheduler()
            .candidate("account-1")
            .unwrap()
            .cooldowns["gpt-test"];
        assert!(retry_at >= before + 120_000, "stream={stream}");
        assert!(retry_at <= runtime_now_ms() + 120_000);
        let events = crate::poison::mutex(&events);
        assert_eq!(
            events[0].routing.as_ref().unwrap().attempt_outcome,
            Some(crate::AttemptOutcome::ToolDiscovery)
        );
        let event = &events[1];
        assert!(!event.success);
        assert_eq!(event.http_status, 429);
        let details = event.upstream_error.as_ref().unwrap();
        assert_eq!(details.http_status, Some(429));
        assert_eq!(details.code.as_deref(), Some("rate_limit_exceeded"));
        assert_eq!(details.error_type.as_deref(), Some("tokens"));
        assert_eq!(
            details.request_id.as_deref(),
            Some("req_synthetic_discovery_limit")
        );
        let message = details.message.as_deref().unwrap();
        assert!(message.contains("TPM limit reached"));
        assert!(details.redacted);
        assert!(!message.contains("synthetic-private@example.invalid"));
        assert!(!message.contains("https://example.invalid/private"));
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
