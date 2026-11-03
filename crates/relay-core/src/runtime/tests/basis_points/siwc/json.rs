use super::*;

fn completed_response() -> Value {
    json!({"id":"resp_synthetic_json","object":"response","status":"completed",
        "model":"gpt-test","output":[{
            "id":"msg_synthetic_json","type":"message","role":"assistant",
            "status":"completed","content":[{"type":"output_text",
                "text":"Synthetic JSON answer","annotations":[]}]
        }],"usage":{"input_tokens":41,"output_tokens":7,"total_tokens":48}})
}

#[tokio::test]
async fn siwc_json_preserves_native_and_converted_client_streams() {
    for (chat, debug) in [(false, false), (true, false), (false, true), (true, true)] {
        for stream in [false, true] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            *crate::poison::mutex(&siwc.state.generation_body) = completed_response();
            let usage = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = usage.clone();
            let mut runtime = runtime(&siwc, &native).await;
            let settings = Arc::get_mut(&mut runtime).unwrap();
            settings.diagnostics_enabled = if debug { || true } else { || false };
            settings.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
            let (base, task) = recovery::gateway(runtime).await;
            let (path, request) = if chat {
                (
                    "chat/completions",
                    json!({"model":"gpt-test","stream":stream,
                    "messages":[{"role":"user","content":"Synthetic input"}]}),
                )
            } else {
                (
                    "responses",
                    json!({"model":"gpt-test","stream":stream,
                    "input":"Synthetic input"}),
                )
            };
            let response = retry_admission::bounded_client()
                .post(format!("{base}/v1/{path}"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let content_type = response.headers()["content-type"].clone();
            let text = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{text}");
            assert!(text.contains("Synthetic JSON answer"), "{text}");
            if stream {
                assert_eq!(content_type, "text/event-stream");
                let events: Vec<Value> = text
                    .lines()
                    .filter_map(|line| serde_json::from_str(line.strip_prefix("data: ")?).ok())
                    .collect();
                if chat {
                    assert!(
                        events
                            .iter()
                            .any(|event| event["choices"][0]["finish_reason"] == "stop")
                    );
                    assert!(!text.contains("response.completed"));
                } else {
                    let terminal: Vec<_> = events
                        .iter()
                        .filter(|event| event["type"] == "response.completed")
                        .collect();
                    assert_eq!(terminal.len(), 1);
                    assert_eq!(terminal[0]["response"], completed_response());
                }
            } else {
                assert_eq!(content_type, "application/json");
                let body: Value = serde_json::from_str(&text).unwrap();
                if chat {
                    assert_eq!(body["choices"][0]["finish_reason"], "stop");
                } else {
                    assert_eq!(body, completed_response());
                }
            }
            let usage = crate::poison::mutex(&usage);
            assert_eq!(usage.len(), 1);
            assert!(usage[0].success);
            assert_eq!(usage[0].input_tokens, Some(41));
            assert_eq!(usage[0].output_tokens, Some(7));
            assert_eq!(usage[0].ttft_ms, None);
            assert_eq!(usage[0].generation_ms, None);
            let transport = usage[0].routing.as_ref().unwrap().transport.as_ref();
            assert_eq!(transport.is_some(), debug);
            if debug {
                let transport = transport.unwrap();
                assert_eq!(transport.upstream_response_ms, Some(usage[0].latency_ms));
                assert!(transport.client_response_ready_ms.unwrap() >= usage[0].latency_ms);
            }
            assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_json_incomplete_retains_partial_output_and_usage_without_replay() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        let mut partial = completed_response();
        partial["status"] = json!("incomplete");
        partial["incomplete_details"] = json!({"reason":"max_output_tokens"});
        *crate::poison::mutex(&siwc.state.generation_body) = partial.clone();
        let usage = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = usage.clone();
        let mut runtime = runtime(&siwc, &native).await;
        Arc::get_mut(&mut runtime).unwrap().usage =
            Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        runtime.set_route_recovery_enabled(true);
        let (base, task) = recovery::gateway(runtime).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"Synthetic input","stream":stream}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{text}");
        let body = if stream {
            assert!(!text.contains("response.completed"));
            let terminal: Vec<Value> = text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line.strip_prefix("data: ")?).ok())
                .filter(|event| event["type"] == "response.incomplete")
                .collect();
            assert_eq!(terminal.len(), 1);
            terminal[0]["response"].clone()
        } else {
            serde_json::from_str(&text).unwrap()
        };
        assert_eq!(body, partial);
        let usage = crate::poison::mutex(&usage);
        // A logical completion projection is not another physical attempt.
        let attempts: Vec<_> = usage
            .iter()
            .filter(|event| !event.is_request_completion())
            .collect();
        assert_eq!(attempts.len(), 1);
        assert!(usage.iter().all(|event| !event.success));
        assert_eq!(attempts[0].input_tokens, Some(41));
        assert_eq!(attempts[0].output_tokens, Some(7));
        assert_eq!(
            attempts[0].error_category.as_deref(),
            Some(crate::error_codes::RESPONSE_INCOMPLETE)
        );
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_large_json_errors_and_invalid_responses_never_replay() {
    for stream in [false, true] {
        for case in ["error", "failed", "invalid", "model"] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            let error = json!({"code":"synthetic_provider_error","message":"Synthetic refusal"});
            let mut body = match case {
                "error" => json!({"error":error}),
                "failed" => json!({"id":"resp_synthetic_failed","status":"failed",
                    "output":[],"error":error}),
                "model" => {
                    let mut response = completed_response();
                    response["model"] = json!("wrong-synthetic-model");
                    response
                }
                _ => json!({"id":"resp_synthetic_unfinished","status":"in_progress","output":[]}),
            };
            body["synthetic_padding"] = json!("x".repeat(95_000));
            *crate::poison::mutex(&siwc.state.generation_body) = body;
            let runtime = runtime(&siwc, &native).await;
            runtime.set_route_recovery_enabled(true);
            let (base, task) = recovery::gateway(runtime).await;
            let response = retry_admission::bounded_client()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&json!({"model":"gpt-test","input":"Synthetic input","stream":stream}))
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert!(!status.is_success(), "{case}: {text}");
            assert!(!text.contains("relay_stream_eof"), "{case}: {text}");
            if matches!(case, "error" | "failed") {
                assert!(text.contains("synthetic_provider_error"), "{case}: {text}");
            }
            assert_eq!(
                siwc.state.generation_requests.load(Ordering::SeqCst),
                1,
                "{case}"
            );
            assert_eq!(
                native.state.generation_requests.load(Ordering::SeqCst),
                0,
                "{case}"
            );
            task.abort();
        }
    }
}
