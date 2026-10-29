use super::*;

fn messages_request(stream: bool) -> Value {
    let schema = json!({"type":"object","properties":{"path":{
        "type":"string","description":"Synthetic field contract. ".repeat(60)
    }},"required":["path"],"additionalProperties":false});
    json!({
        "model":crate::claude_desktop_model_alias("gpt-test"),"stream":stream,"max_tokens":128,
        "messages":[{"role":"user","content":"Inspect the synthetic file"}],
        "tools":(0..24).map(|i| json!({"name":format!("lookup_{i}"),"input_schema":schema})).collect::<Vec<_>>()
    })
}

fn messages_tool_use(body: &str, stream: bool) -> Value {
    if !stream {
        let reply: Value = serde_json::from_str(body).unwrap();
        assert_eq!(reply["stop_reason"], "tool_use");
        return reply["content"]
            .as_array()
            .unwrap()
            .iter()
            .find(|block| block["type"] == "tool_use")
            .unwrap()
            .clone();
    }
    let mut tool = None;
    let mut input = String::new();
    let mut stopped = false;
    for line in body.lines().filter_map(|line| line.strip_prefix("data: ")) {
        let event: Value = serde_json::from_str(line).unwrap();
        assert_ne!(event["type"], "error", "{event}");
        if event["type"] == "content_block_start" && event["content_block"]["type"] == "tool_use" {
            assert!(tool.is_none(), "only one client call is released");
            tool = Some(event["content_block"].clone());
        }
        if event["delta"]["type"] == "input_json_delta" {
            input.push_str(event["delta"]["partial_json"].as_str().unwrap());
        }
        if event["type"] == "message_delta" {
            assert_eq!(event["delta"]["stop_reason"], "tool_use");
        }
        stopped |= event["type"] == "message_stop";
    }
    assert!(stopped);
    let mut tool = tool.unwrap();
    tool["input"] = serde_json::from_str(&input).unwrap();
    tool
}

#[tokio::test]
async fn messages_legacy_catalog_identity_preserves_json_sse_and_tool_result_roundtrip() {
    for stream in [false, true] {
        for mixed in [false, true] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            server
                .state
                .stream_generations
                .store(true, Ordering::SeqCst);
            server
                .state
                .strict_request_schema
                .store(true, Ordering::SeqCst);
            let mut lookup = tool_response("relay.catalog", "call_catalog", json!({}));
            lookup["output"][0]["arguments"] = json!({"code":json!({
                "name":"relay.catalog","arguments":{"names":["lookup_0"]}
            }).to_string()})
            .to_string()
            .into();
            let call = tool_response("lookup_0", "call_client", json!({"path":"synthetic.txt"}));
            if mixed {
                lookup["output"]
                    .as_array_mut()
                    .unwrap()
                    .push(call["output"][0].clone());
            }
            crate::poison::mutex(&server.state.generation_replies)
                .push_back((StatusCode::OK, lookup));
            if !mixed {
                crate::poison::mutex(&server.state.generation_replies)
                    .push_back((StatusCode::OK, call));
            }
            *crate::poison::mutex(&server.state.generation_body) = final_response();
            let (base, task) = recovery::gateway(Arc::new(runtime(&server).await)).await;
            let client = reqwest::Client::new();
            let mut request = messages_request(stream);
            let reply = client
                .post(format!("{base}/v1/messages"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), StatusCode::OK);
            let body = reply.text().await.unwrap();
            assert!(!body.contains("relay.catalog"), "{body}");
            let tool = messages_tool_use(&body, stream);
            assert_eq!(tool["name"], "lookup_0");
            assert_eq!(tool["input"], json!({"path":"synthetic.txt"}));
            let sends = if mixed { 1 } else { 2 };
            assert_eq!(
                server.state.generation_requests.load(Ordering::SeqCst),
                sends
            );
            request["stream"] = json!(false);
            request["messages"].as_array_mut().unwrap().extend([
                json!({"role":"assistant","content":[tool.clone()]}),
                json!({"role":"user","content":[{"type":"tool_result","tool_use_id":tool["id"],"content":"Synthetic file contents"}]}),
            ]);
            let reply = client
                .post(format!("{base}/v1/messages"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), StatusCode::OK);
            let body: Value = reply.json().await.unwrap();
            assert_eq!(body["content"][0]["text"], "Ready");
            assert_eq!(
                server.state.generation_requests.load(Ordering::SeqCst),
                sends + 1
            );
            let bodies = crate::poison::mutex(&server.state.generation_bodies);
            let input = bodies.last().unwrap()["input"].as_array().unwrap();
            let result = input
                .iter()
                .find(|item| {
                    item["type"] == "function_call_output"
                        && item["output"] == "Synthetic file contents"
                })
                .unwrap();
            assert!(input.iter().any(
                |item| item["type"] == "function_call" && item["call_id"] == result["call_id"]
            ));
            task.abort();
        }
    }
}

#[tokio::test]
async fn unknown_tool_identity_reaches_messages_json_and_sse_without_retry_or_payload() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        *crate::poison::mutex(&server.state.generation_body) = tool_response(
            "synthetic-private-tool",
            "call_private",
            json!({"path":"synthetic-private-value"}),
        );
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = runtime(&server).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let reply = reqwest::Client::new()
            .post(format!("{base}/v1/messages"))
            .bearer_auth("local-secret")
            .json(&messages_request(stream))
            .send()
            .await
            .unwrap();
        if !stream {
            assert_eq!(reply.status(), StatusCode::BAD_GATEWAY);
        }
        let body = reply.text().await.unwrap();
        assert!(body.contains("tool_identity_reason=unknown_tool"), "{body}");
        assert!(body.contains("repair_skipped=executable_tool"));
        assert!(!body.contains("synthetic-private"));
        assert!(!body.contains("\"type\":\"tool_use\""));
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 1);
        assert!(!events[0].success);
        let diagnostic = serde_json::to_string(&events[0]).unwrap();
        assert!(diagnostic.contains("tool_identity_reason=unknown_tool"));
        assert!(!diagnostic.contains("synthetic-private"));
        task.abort();
    }
}
