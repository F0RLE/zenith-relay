use super::*;

fn format() -> Value {
    json!({"type":"json_schema","name":"synthetic_result","strict":false,
        "schema":{"type":"object","properties":{"approved":{"type":"boolean"}},
            "required":["approved"],"additionalProperties":false}})
}

fn reply(text: &str, completed: bool) -> Value {
    json!({"id":"synthetic-result","model":"gpt-test",
        "status":if completed {"completed"} else {"incomplete"},
        "output":[{"type":"message","id":"synthetic-message","role":"assistant",
            "content":[{"type":"output_text","text":text}]}]})
}

fn request(client: WireApi, stream: bool) -> (&'static str, Value) {
    match client {
        WireApi::Responses => (
            "/v1/responses",
            json!({"model":"gpt-test","input":"Synthetic input","stream":stream,
                "text":{"format":format()}}),
        ),
        _ => {
            let mut schema = format();
            schema.as_object_mut().unwrap().remove("type");
            (
                "/v1/chat/completions",
                json!({"model":"gpt-test","messages":[{"role":"user","content":"Synthetic input"}],
                    "stream":stream,"response_format":{"type":"json_schema","json_schema":schema}}),
            )
        }
    }
}

#[tokio::test]
async fn non_strict_schema_roundtrips_json_and_sse_without_a_private_option_or_retries() {
    for client in [WireApi::Responses, WireApi::ChatCompletions] {
        for stream in [false, true] {
            for upstream_stream in [false, true] {
                for (text, completed, valid) in [
                    ("{\"approved\":false}", true, true),
                    ("{\"approved\":\"yes\"}", true, false),
                    ("allow", true, false),
                    ("{\"approved\":false}", false, false),
                ] {
                    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
                    *crate::poison::mutex(&server.state.generation_body) = reply(text, completed);
                    server
                        .state
                        .stream_generations
                        .store(upstream_stream, Ordering::SeqCst);
                    server
                        .state
                        .strict_request_schema
                        .store(true, Ordering::SeqCst);
                    let runtime = Arc::new(runtime(&server).await);
                    let (base, task) = super::recovery::gateway(runtime).await;
                    let (path, request) = request(client, stream);
                    let response = super::retry_admission::bounded_client()
                        .post(format!("{base}{path}"))
                        .bearer_auth("local-secret")
                        .json(&request)
                        .send()
                        .await
                        .unwrap();
                    let status = response.status();
                    let body = response.text().await.unwrap();
                    if valid {
                        assert_eq!(status, StatusCode::OK, "{body}");
                        if !stream {
                            let body: Value = serde_json::from_str(&body).unwrap();
                            let answer = match client {
                                WireApi::Responses => &body["output"][0]["content"][0]["text"],
                                _ => &body["choices"][0]["message"]["content"],
                            };
                            assert_eq!(answer, text);
                        } else {
                            assert!(body.contains("approved"), "{body}");
                            assert!(body.contains("false"), "{body}");
                        }
                    } else {
                        assert!(!status.is_success(), "unvalidated output escaped: {body}");
                        assert!(!body.contains("response.output_text.delta"), "{body}");
                        assert!(!body.contains("response.completed"), "{body}");
                    }
                    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
                    {
                        let bodies = crate::poison::mutex(&server.state.generation_bodies);
                        assert_eq!(bodies.len(), 1);
                        assert!(bodies[0].get("text").is_none());
                        assert!(bodies[0].get("relay_bps_output_validation").is_none());
                        let input = bodies[0]["input"].to_string();
                        assert!(input.contains("not native constrained decoding"));
                    }
                    task.abort();
                }
            }
        }
    }
}

#[tokio::test]
async fn non_strict_schema_withholds_stream_headers_until_terminal_validation() {
    for (valid, discovery) in [(false, false), (true, false), (false, true), (true, true)] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        *crate::poison::mutex(&server.state.generation_body) = reply(
            if valid {
                "{\"approved\":false}"
            } else {
                "allow"
            },
            true,
        );
        server
            .state
            .stream_generations
            .store(true, Ordering::SeqCst);
        let gate = Arc::new(Semaphore::new(0));
        crate::poison::mutex(&server.state.stream_gates).push_back(gate.clone());
        let runtime = Arc::new(runtime(&server).await);
        let (base, task) = super::recovery::gateway(runtime).await;
        let (path, mut request) = request(WireApi::Responses, true);
        if discovery {
            request["relay_bps_tool_catalog"] = json!("discovery");
            request["tools"] = json!([{"type":"function","name":"inspect",
                "parameters":{"type":"object","properties":{},"additionalProperties":false}}]);
        }
        let client = super::retry_admission::bounded_client();
        let mut response = Box::pin(
            client
                .post(format!("{base}{path}"))
                .bearer_auth("local-secret")
                .json(&request)
                .send(),
        );
        tokio::select! {
            response = &mut response => panic!("stream released before validation: {response:?}"),
            () = tokio::time::sleep(Duration::from_millis(150)) => {}
        }
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        gate.add_permits(1);
        let response = response.await.unwrap();
        assert_eq!(response.status().is_success(), valid);
        let body = response.text().await.unwrap();
        assert_eq!(body.contains("response.completed"), valid, "{body}");
        task.abort();
    }
}

#[tokio::test]
async fn non_strict_schema_preserves_a_tool_result_and_validates_the_next_answer() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    crate::poison::mutex(&server.state.generation_replies).extend([
        (
            StatusCode::OK,
            json!({"id":"synthetic-tool-response","model":"gpt-test",
            "status":"completed","output":[{"id":"synthetic-call-item",
                "type":"function_call","name":"run_officejs","status":"completed",
                "call_id":"synthetic-call","arguments":json!({"references":["inspect"],
                    "code":"{\"path\":\"synthetic.txt\"}"}).to_string()}]}),
        ),
        (StatusCode::OK, reply("{\"approved\":false}", true)),
    ]);
    let runtime = Arc::new(runtime(&server).await);
    let (base, task) = super::recovery::gateway(runtime).await;
    let (path, mut request) = request(WireApi::Responses, false);
    request["tools"] = json!([{"type":"function","name":"inspect",
        "parameters":{"type":"object","properties":{"path":{"type":"string"}},
            "required":["path"],"additionalProperties":false}}]);
    let client = super::retry_admission::bounded_client();
    let response = client
        .post(format!("{base}{path}"))
        .bearer_auth("local-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let first: Value = response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{first}");
    let call = first["output"][0].clone();
    assert_eq!(call["name"], "inspect");
    assert_eq!(
        serde_json::from_str::<Value>(call["arguments"].as_str().unwrap()).unwrap(),
        json!({"path":"synthetic.txt"})
    );
    request["input"] = json!([{"role":"user","content":"Synthetic input"}, call,
        {"type":"function_call_output","call_id":call["call_id"],"output":"Synthetic result"}]);
    let response = client
        .post(format!("{base}{path}"))
        .bearer_auth("local-secret")
        .json(&request)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let second: Value = response.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(
        second["output"][0]["content"][0]["text"],
        "{\"approved\":false}"
    );
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
    let bodies = crate::poison::mutex(&server.state.generation_bodies);
    assert!(
        bodies[1]["input"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["type"] == "function_call_output"
                && item["output"] == "Synthetic result")
    );
    task.abort();
}
