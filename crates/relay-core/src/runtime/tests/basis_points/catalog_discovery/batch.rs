use super::*;

pub(super) fn batch_response() -> Value {
    let mut reply = tool_response("lookup", "call_batch", json!({"path":"a"}));
    reply["output"][0]["arguments"] = json!({"references":["lookup","lookup"],"code":"relay-tool-batch-v1\n[{\"path\":\"a\"},{\"path\":\"b\"}]"}).to_string().into();
    reply
}

fn client_response(bytes: &[u8], stream: bool) -> Value {
    if !stream {
        return serde_json::from_slice(bytes).unwrap();
    }
    let mut pending = bytes.to_vec();
    while let Some(frame) = crate::protocol::take_sse_event(&mut pending) {
        if let Ok(event) = serde_json::from_slice::<Value>(&crate::protocol::sse_data(&frame)) {
            assert_ne!(event["type"], "error", "{event}");
            if event["type"] == "response.completed" {
                return event["response"].clone();
            }
        }
    }
    panic!("missing authoritative terminal");
}

#[tokio::test]
async fn native_batch_roundtrips_json_sse_and_discovery_without_regeneration() {
    for stream in [false, true] {
        for mode in [
            "compact",
            "lookup_then_batch",
            "mixed_lookup",
            "interleaved_batches",
            "crlf_batch",
            "literal_batch",
            "interrupted_results",
        ] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            server
                .state
                .strict_request_schema
                .store(true, Ordering::SeqCst);
            server
                .state
                .stream_generations
                .store(stream, Ordering::SeqCst);
            let lookup =
                tool_response("relay.catalog", "call_catalog", json!({"names":["lookup"]}));
            let mut batch = batch_response();
            let code = match mode {
                "crlf_batch" => Some("relay-tool-batch-v1\r\n[{\"path\":\"a\"},{\"path\":\"b\"}]"),
                "literal_batch" => Some(
                    "relay-tool-batch-v1\n[{\"path\":\"a\nline\"},{\"path\":\"C:\\synthetic\\q\"}]",
                ),
                _ => None,
            };
            if let Some(code) = code {
                batch["output"][0]["arguments"] =
                    json!({"references":["lookup","lookup"],"code":code})
                        .to_string()
                        .into();
            }
            let reasoning = json!({"type":"reasoning","id":"rs_synthetic","summary":[],"encrypted_content":"synthetic-opaque"});
            batch["output"]
                .as_array_mut()
                .unwrap()
                .insert(0, reasoning.clone());
            let interleaved = mode == "interleaved_batches";
            let interrupted = mode == "interrupted_results";
            if interleaved {
                let mut second = batch_response()["output"][0].clone();
                second["call_id"] = json!("call_batch_second");
                second["id"] = json!("fc_batch_second");
                batch["output"].as_array_mut().unwrap().extend([
                    second,
                    tool_response("lookup", "call_single", json!({"path":"c"}))["output"][0]
                        .clone(),
                ]);
            }
            if mode == "mixed_lookup" {
                batch["output"]
                    .as_array_mut()
                    .unwrap()
                    .push(lookup["output"][0].clone());
            }
            let generations = if mode == "lookup_then_batch" { 2 } else { 1 };
            {
                let mut replies = crate::poison::mutex(&server.state.generation_replies);
                if mode == "lookup_then_batch" {
                    replies.push_back((StatusCode::OK, lookup));
                }
                replies.push_back((StatusCode::OK, batch));
                replies.push_back((StatusCode::OK, final_response()));
            }
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let mut runtime = runtime(&server).await;
            runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let (base, task) = recovery::gateway(Arc::new(runtime)).await;
            let mut request = catalog_request(stream);
            request["parallel_tool_calls"] = json!(true);
            request["relay_bps_tool_catalog"] = json!(if mode == "compact" {
                "compact"
            } else {
                "discovery"
            });
            let client = reqwest::Client::new();
            let reply = client
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), StatusCode::OK);
            let body = client_response(&reply.bytes().await.unwrap(), stream);
            let calls = body["output"].as_array().unwrap();
            assert_eq!(calls.len(), if interleaved { 6 } else { 3 });
            assert_eq!(calls[0], reasoning);
            assert_ne!(calls[1]["call_id"], calls[2]["call_id"]);
            assert_eq!(calls[1]["name"], "lookup");
            assert_eq!(calls[2]["name"], "lookup");
            for (call, path) in calls[1..3].iter().zip(if mode == "literal_batch" {
                ["a\nline", "C:\\synthetic\\q"]
            } else {
                ["a", "b"]
            }) {
                assert_eq!(
                    serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
                    json!({"path":path})
                );
            }
            assert_eq!(
                server.state.generation_requests.load(Ordering::SeqCst),
                generations
            );
            {
                let events = crate::poison::mutex(&events);
                assert_eq!(events.len(), generations);
                let event = events.last().unwrap();
                assert!(event.success);
                assert_eq!(
                    event.tool_use.upstream_tool_call_count,
                    Some(if interleaved {
                        3
                    } else if mode == "mixed_lookup" {
                        2
                    } else {
                        1
                    })
                );
                assert_eq!(
                    event.tool_use.tool_call_count,
                    if interleaved { 5 } else { 2 }
                );
                assert!(event.tool_use.tool_calls_ready_ms.is_some());
            }
            let mut history = vec![json!({"role":"user","content":"Inspect synthetic files"})];
            let boundary = json!({"type":"message","role":"developer","content":[{"type":"input_text","text":"Synthetic service instruction between tool results"}]});
            history.extend(calls.clone());
            let order = if interleaved {
                vec![2, 4, 5, 1, 3]
            } else {
                vec![2, 1]
            };
            for index in order {
                history.push(json!({"type":"function_call_output","call_id":calls[index]["call_id"],"output":format!("synthetic result-{index}")}));
                if interrupted && index == 2 {
                    history.push(boundary.clone());
                }
            }
            request["input"] = json!(history);
            let reply = client
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            assert_eq!(reply.status(), StatusCode::OK);
            assert_eq!(
                client_response(&reply.bytes().await.unwrap(), stream)["output"][0]["content"][0]["text"],
                "Ready"
            );
            assert_eq!(
                server.state.generation_requests.load(Ordering::SeqCst),
                generations + 1
            );
            let bodies = crate::poison::mutex(&server.state.generation_bodies);
            let input = bodies.last().unwrap()["input"].as_array().unwrap();
            assert!(input.contains(&reasoning));
            if interrupted {
                let position = input.iter().position(|item| item == &boundary).unwrap();
                assert_eq!(input[position - 1]["output"], "synthetic result-2");
                assert_eq!(input[position + 1]["output"], "synthetic result-1");
                let replayed: Vec<_> = input
                    .iter()
                    .filter(|item| item["type"] == "function_call")
                    .collect();
                assert_eq!(replayed.len(), 2);
                assert_eq!(input[position - 1]["call_id"], replayed[1]["call_id"]);
                assert_eq!(input[position + 1]["call_id"], replayed[0]["call_id"]);
                assert_ne!(replayed[0]["call_id"], replayed[1]["call_id"]);
                for (call, original) in replayed.iter().zip(&calls[1..]) {
                    assert!(call["id"]
                        .as_str()
                        .unwrap()
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')));
                    let envelope: Value =
                        serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
                    assert_eq!(envelope["references"], json!(["lookup"]));
                    assert_eq!(
                        serde_json::from_str::<Value>(envelope["code"].as_str().unwrap()).unwrap(),
                        serde_json::from_str::<Value>(original["arguments"].as_str().unwrap())
                            .unwrap()
                    );
                }
                task.abort();
                continue;
            }
            let call = input
                .iter()
                .find(|item| item["type"] == "function_call")
                .unwrap();
            assert_eq!(call["call_id"], "call_batch");
            let envelope: Value =
                serde_json::from_str(call["arguments"].as_str().unwrap()).unwrap();
            assert_eq!(envelope["references"], json!(["lookup", "lookup"]));
            let result = input
                .iter()
                .find(|item| item["type"] == "function_call_output")
                .unwrap();
            assert_eq!(result["call_id"], "call_batch");
            assert_eq!(result["output"].as_array().unwrap().len(), 4);
            assert_eq!(result["output"][1]["text"], "synthetic result-1");
            assert_eq!(result["output"][3]["text"], "synthetic result-2");
            if interleaved {
                let results: Vec<_> = input
                    .iter()
                    .filter(|item| item["type"] == "function_call_output")
                    .collect();
                assert_eq!(results.len(), 3);
                assert_eq!(results[1]["call_id"], "call_batch_second");
                assert_eq!(results[1]["output"][1]["text"], "synthetic result-3");
                assert_eq!(results[1]["output"][3]["text"], "synthetic result-4");
                assert_eq!(results[2]["call_id"], "call_single");
                assert_eq!(results[2]["output"], "synthetic result-5");
            }
            task.abort();
        }
    }
}

#[tokio::test]
async fn invalid_native_batch_records_no_ready_tools_and_never_retries() {
    for stream in [false, true] {
        for failure in [
            "schema",
            "serial",
            "in_progress",
            "incomplete",
            "catalog_in_progress",
            "catalog_incomplete",
        ] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            server
                .state
                .stream_generations
                .store(stream, Ordering::SeqCst);
            let mut batch = batch_response();
            match failure {
                "schema" => {
                    batch["output"][0]["arguments"] = json!({"references":["lookup","lookup"],"code":"relay-tool-batch-v1\n[{\"path\":\"valid\"},{\"path\":0}]"}).to_string().into();
                }
                "serial" => {}
                status => {
                    let status = if let Some(status) = status.strip_prefix("catalog_") {
                        batch = tool_response(
                            "relay.catalog",
                            "call_catalog",
                            json!({"names":["lookup"]}),
                        );
                        status
                    } else {
                        status
                    };
                    batch["output"][0]["status"] = json!(status);
                }
            }
            crate::poison::mutex(&server.state.generation_replies)
                .push_back((StatusCode::OK, batch));
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let mut runtime = runtime(&server).await;
            runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let (base, task) = recovery::gateway(Arc::new(runtime)).await;
            let mut request = catalog_request(stream);
            request["parallel_tool_calls"] = json!(failure != "serial");
            let reply = reqwest::Client::new()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let bytes = reply.bytes().await.unwrap();
            assert!(!String::from_utf8_lossy(&bytes).contains("\"name\":\"lookup\""));
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
            let events = crate::poison::mutex(&events);
            assert_eq!(events.len(), 1);
            assert!(!events[0].success);
            assert_eq!(events[0].tool_use.upstream_tool_call_count, Some(1));
            assert_eq!(events[0].tool_use.tool_call_count, 0);
            assert!(events[0].tool_use.tool_calls_ready_ms.is_none());
            task.abort();
        }
    }
}
