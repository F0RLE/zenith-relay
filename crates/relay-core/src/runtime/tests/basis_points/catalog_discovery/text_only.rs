use super::*;

#[tokio::test]
async fn text_only_prefix_roundtrip_is_scoped_and_never_authorizes_tools() {
    for (stream, batched) in [(false, false), (true, false), (false, true), (true, true)] {
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
                tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]})),
            ),
            (
                StatusCode::OK,
                if batched {
                    super::batch::batch_response()
                } else {
                    tool_response("lookup", "call_client", json!({"path":"synthetic.txt"}))
                },
            ),
        ]);
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let client = reqwest::Client::new();
        let mut request = catalog_request(false);
        request["instructions"] = json!("Preserve synthetic project constraints.");
        request["reasoning"] = json!({"effort":"xhigh"});
        request["prompt_cache_key"] = json!("synthetic-cache-key");
        request["input"] = json!([
            {"role":"developer","content":"Synthetic project rules"},
            {"role":"user","content":"Inspect the synthetic file"}
        ]);
        let response = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .header("x-codex-session-id", "synthetic-root")
            .header("thread-id", "thread-a")
            .json(&request)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["output"][0]["name"], "lookup");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
        let initial = crate::poison::mutex(&server.state.generation_bodies)[0].clone();
        let prefix = initial["input"][1].clone();
        assert!(prefix["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("relay.catalog"));

        *crate::poison::mutex(&server.state.generation_body) = final_response();
        let mut next = request;
        if stream {
            next["tools"] = json!([]);
        } else {
            next.as_object_mut().unwrap().remove("tools");
        }
        next["stream"] = json!(stream);
        let calls = body["output"].as_array().unwrap();
        next["input"].as_array_mut().unwrap().extend(calls.clone());
        for call in calls.iter().rev() {
            next["input"].as_array_mut().unwrap().push(json!({
                "type":"function_call_output","call_id":call["call_id"],"output":"Synthetic file contents"
            }));
        }
        next["input"]
            .as_array_mut()
            .unwrap()
            .push(json!({"role":"user","content":"Summarize this synthetic conversation"}));
        for (thread, reuse, changed) in [
            (Some("thread-a"), true, false),
            (Some("thread-b"), false, false),
            (None, false, false),
            (Some("thread-a"), false, true),
        ] {
            let mut current = next.clone();
            if changed {
                current["instructions"] = json!("Different synthetic project constraints.");
            }
            let before = server.state.generation_requests.load(Ordering::SeqCst);
            let mut send = client
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret");
            if let Some(thread) = thread {
                send = send
                    .header("x-codex-session-id", "synthetic-root")
                    .header("thread-id", thread);
            }
            let response = send.json(&current).send().await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let text = response.text().await.unwrap();
            assert!(text.contains("Ready"));
            assert!(!text.contains("\"type\":\"function_call\""));
            assert_eq!(
                server.state.generation_requests.load(Ordering::SeqCst),
                before + 1,
                "a tool-less continuation must not start discovery"
            );
            let bodies = crate::poison::mutex(&server.state.generation_bodies);
            let sent = bodies.last().unwrap();
            assert_eq!(sent["input"][1] == prefix, reuse);
            assert_eq!(sent["reasoning_effort"], initial["reasoning_effort"]);
            assert_eq!(sent["prompt_cache_key"], initial["prompt_cache_key"]);
            assert!(sent.get("tools").is_none());
            if reuse {
                let input = sent["input"].as_array().unwrap();
                assert_eq!(&input[..4], initial["input"].as_array().unwrap());
                assert_eq!(
                    input[4]["call_id"],
                    if batched { "call_batch" } else { "call_client" }
                );
                if batched {
                    assert_eq!(input[5]["output"][1]["text"], "Synthetic file contents");
                    assert_eq!(input[5]["output"][3]["text"], "Synthetic file contents");
                } else {
                    assert_eq!(input[5]["output"], "Synthetic file contents");
                }
                assert!(input.last().unwrap()["content"][0]["text"]
                    .as_str()
                    .unwrap()
                    .contains("No client tools are available"));
            }
            let events = crate::poison::mutex(&events);
            let event = events.last().unwrap();
            assert!(event.success);
            assert_eq!(event.tool_use.client_tool_count, 0);
            assert_eq!(event.tool_use.forwarded_tool_count, 0);
            assert_eq!(
                event
                    .tool_use
                    .basis_points_catalog
                    .as_ref()
                    .unwrap()
                    .discovery_rounds,
                0
            );
        }

        *crate::poison::mutex(&server.state.generation_body) =
            tool_response("lookup", "call_forbidden", json!({"path":"synthetic.txt"}));
        let before = server.state.generation_requests.load(Ordering::SeqCst);
        let response = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .header("x-codex-session-id", "synthetic-root")
            .header("thread-id", "thread-a")
            .json(&next)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            if stream {
                StatusCode::OK
            } else {
                StatusCode::BAD_GATEWAY
            }
        );
        let text = response.text().await.unwrap();
        assert!(text.contains(crate::error_codes::ADAPTER_UPSTREAM_RESPONSE_INVALID));
        assert!(!text.contains("\"type\":\"function_call\""));
        assert_eq!(
            server.state.generation_requests.load(Ordering::SeqCst),
            before + 1
        );
        assert_eq!(
            crate::poison::mutex(&server.state.generation_bodies)
                .last()
                .unwrap()["input"][1],
            prefix
        );
        task.abort();
    }
}
