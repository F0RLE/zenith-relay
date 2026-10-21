use super::*;

async fn runtime(siwc: &AccessServer, native: &AccessServer) -> Arc<GatewayRuntime> {
    let mut runtime = retry_admission::pool_runtime(siwc, 1, true).await;
    let account = runtime
        .chatgpt_accounts
        .get_mut("synthetic-account-0")
        .unwrap();
    account.oauth_client_kind = OAuthClientKind::Siwc;
    account.responses_url = siwc.responses_url.clone();
    runtime
        .chatgpt_accounts
        .get_mut("synthetic-account-1")
        .unwrap()
        .responses_url = native.responses_url.clone();
    Arc::new(runtime)
}

#[tokio::test]
async fn siwc_uses_sse_upstream_for_both_client_modes_and_preserves_tool_history() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        *crate::poison::mutex(&siwc.state.generation_body) = json!({
            "id":"synthetic-siwc-response","status":"completed","output":[{
                "type":"message","role":"assistant","content":[{
                    "type":"output_text","text":"Synthetic SIWC answer"
                }]
            }]
        });
        let runtime = runtime(&siwc, &native).await;
        let (base, task) = recovery::gateway(runtime).await;
        let input = json!([
            {"type":"function_call","name":"lookup","call_id":"call_synthetic","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_synthetic","output":"Synthetic result"}
        ]);
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(
                &json!({"model":"gpt-test","input":input,"stream":stream,"tools":[{
                    "type":"function","name":"lookup","parameters":{"type":"object"}
                }]}),
            )
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{text}");
        assert!(text.contains("Synthetic SIWC answer"), "{text}");
        if stream {
            assert!(text.contains("response.completed"), "{text}");
        } else {
            assert_eq!(
                serde_json::from_str::<Value>(&text).unwrap()["status"],
                "completed"
            );
        }
        let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
        assert_eq!(bodies.len(), 1);
        assert_eq!(bodies[0]["stream"], true);
        assert_eq!(bodies[0]["store"], false);
        assert_eq!(bodies[0]["input"][0]["type"], "additional_tools");
        assert_eq!(bodies[0]["input"][0]["role"], "developer");
        assert_eq!(bodies[0]["input"][1], input[0]);
        assert_eq!(bodies[0]["input"][2], input[1]);
        assert_eq!(siwc.state.access_requests.load(Ordering::SeqCst), 0);
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_compaction_summarizes_without_tools_and_reuses_the_checkpoint() {
    for (endpoint, stream) in [
        ("responses", false),
        ("responses", true),
        ("responses/compact", false),
    ] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        *crate::poison::mutex(&siwc.state.generation_body) = json!({
            "id":"synthetic-siwc-summary","status":"completed","output":[{
                "type":"message","role":"assistant","content":[{
                    "type":"output_text","text":"Keep the synthetic task and its next step."
                }]
            }], "usage":{"input_tokens":40,"output_tokens":10}
        });
        let runtime = runtime(&siwc, &native).await;
        let (base, task) = recovery::gateway(runtime).await;
        let client = retry_admission::bounded_client();
        let mut input = json!([{"role":"user","content":"Synthetic task history"}]);
        if endpoint == "responses" {
            input
                .as_array_mut()
                .unwrap()
                .push(json!({"type":"compaction_trigger"}));
        }
        let response = client
            .post(format!("{base}/v1/{endpoint}"))
            .bearer_auth("local-secret")
            .header("x-codex-session-id", "synthetic-siwc-compaction")
            .json(&json!({"model":"gpt-test","input":input,"stream":stream,
                "tools":[{"type":"function","name":"lookup","parameters":{"type":"object"}}],
                "tool_choice":"auto","parallel_tool_calls":true,
                "prompt_cache_key":"synthetic-siwc-compaction"}))
            .send()
            .await
            .unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        let body = if stream {
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line.strip_prefix("data: ")?).ok())
                .find(|event| event["type"] == "response.completed")
                .expect("completed compaction stream")["response"]
                .clone()
        } else {
            serde_json::from_str::<Value>(&text).unwrap()
        };
        assert_eq!(status, StatusCode::OK, "{endpoint}: {body}");
        if endpoint == "responses/compact" {
            assert_eq!(body["object"], "response.compaction");
        }
        assert_eq!(body["output"][0]["type"], "compaction");
        assert_eq!(body["usage"]["input_tokens"], 40);
        {
            let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
            assert_eq!(bodies.len(), 1);
            assert!(bodies[0].get("tools").is_none());
            assert!(bodies[0].get("tool_choice").is_none());
            assert_eq!(bodies[0]["input"].as_array().unwrap().len(), 2);
            assert_eq!(bodies[0]["stream"], true);
        }
        let response = client
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .header("x-codex-session-id", "synthetic-siwc-compaction")
            .json(
                &json!({"model":"gpt-test","input":[body["output"][0].clone(),
                {"role":"user","content":"Continue the synthetic task"}],"stream":false,
                "prompt_cache_key":"synthetic-siwc-compaction"}),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bodies = crate::poison::mutex(&siwc.state.generation_bodies);
        assert_eq!(bodies.len(), 2);
        let restored = bodies[1]["input"].to_string();
        assert!(restored.contains("Keep the synthetic task"));
        assert!(!restored.contains("encrypted_content"));
        assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_compaction_rejects_foreign_state_and_never_falls_back_after_dispatch() {
    for endpoint in ["responses", "responses/compact"] {
        for foreign_checkpoint in [true, false] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            *crate::poison::mutex(&siwc.state.generation_status) = StatusCode::NOT_FOUND;
            *crate::poison::mutex(&siwc.state.generation_body) =
                json!({"error":{"code":"route_not_found","message":"Synthetic rejection"}});
            let runtime = runtime(&siwc, &native).await;
            runtime.set_route_recovery_enabled(true);
            let (base, task) = recovery::gateway(runtime).await;
            let mut input = vec![json!({"type":"compaction_trigger"})];
            if foreign_checkpoint {
                input.insert(
                    0,
                    json!({"type":"compaction","encrypted_content":"foreign-synthetic"}),
                );
            }
            let response = retry_admission::bounded_client()
                .post(format!("{base}/v1/{endpoint}"))
                .bearer_auth("local-secret")
                .json(&json!({"model":"gpt-test","input":input,"stream":false}))
                .send()
                .await
                .unwrap();
            let expected_status = if foreign_checkpoint {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::NOT_FOUND
            };
            assert_eq!(response.status(), expected_status, "{endpoint}");
            assert_eq!(
                siwc.state.generation_requests.load(Ordering::SeqCst),
                usize::from(!foreign_checkpoint),
                "{endpoint}"
            );
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_quota_rejection_before_output_rotates_to_the_next_pool_account() {
    for client_stream in [false, true] {
        for rejection in ["http", "sse", "sse_message_only"] {
            let exhausted = AccessServer::start(json!([]), false).await;
            let next = AccessServer::start(json!([]), false).await;
            let mut error = json!({
                "code":"subscription_sharing_usage_limit_exceeded",
                "type":"invalid_request_error",
                "message":"Synthetic subscription sharing usage limit exceeded"
            });
            if rejection == "sse_message_only" {
                error.as_object_mut().unwrap().remove("code");
                error["message"] =
                    json!("The ChatGPT user has reached their Subscription Sharing usage limit.");
            }
            if rejection != "http" {
                exhausted
                    .state
                    .stream_generations
                    .store(true, Ordering::SeqCst);
                crate::poison::mutex(&exhausted.state.stream_prefixes).push_back(
                    format!(
                        "data: {}\n\n",
                        json!({"type":"response.failed","response":{
                            "id":"synthetic-quota-rejection","status":"failed","error":error
                        }})
                    )
                    .into_bytes(),
                );
            } else {
                *crate::poison::mutex(&exhausted.state.generation_status) =
                    StatusCode::TOO_MANY_REQUESTS;
                *crate::poison::mutex(&exhausted.state.generation_body) = json!({"error":error});
            }
            next.state.stream_generations.store(true, Ordering::SeqCst);
            *crate::poison::mutex(&next.state.generation_body) = json!({
                "id":"synthetic-next-account","status":"completed","output":[{
                    "type":"message","role":"assistant","content":[{
                        "type":"output_text","text":"Next SIWC pool account answered"
                    }]
                }]
            });
            let mut runtime = runtime(&exhausted, &next).await;
            Arc::get_mut(&mut runtime)
                .unwrap()
                .chatgpt_accounts
                .get_mut("synthetic-account-1")
                .unwrap()
                .oauth_client_kind = OAuthClientKind::Siwc;
            let (base, task) = recovery::gateway(runtime.clone()).await;
            let client = retry_admission::bounded_client();
            for _ in 0..2 {
                let response = client.post(format!("{base}/v1/responses"))
                    .bearer_auth("local-secret")
                    .json(&json!({"model":"gpt-test","input":"Synthetic input","stream":client_stream}))
                    .send().await.unwrap();
                let status = response.status();
                let text = response.text().await.unwrap();
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "stream={client_stream}, refusal={rejection}: {text}"
                );
                assert!(text.contains("Next SIWC pool account answered"), "{text}");
            }
            assert_eq!(
                exhausted.state.generation_requests.load(Ordering::SeqCst),
                1
            );
            assert_eq!(next.state.generation_requests.load(Ordering::SeqCst), 2);
            let order = runtime.candidate_runtime_order_for_key("key-1");
            let waiting = order
                .iter()
                .find(|candidate| candidate.candidate_id == "synthetic-account-0")
                .unwrap();
            assert!(!waiting.available);
            assert!(waiting.quota_wait);
            assert_eq!(
                runtime
                    .lock_scheduler()
                    .candidate("synthetic-account-0")
                    .unwrap()
                    .health,
                CandidateHealth::Healthy
            );
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_quota_after_output_does_not_replay_on_another_account() {
    for stream in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let next = AccessServer::start(json!([]), false).await;
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        let events = [
            json!({"type":"response.output_text.delta","output_index":0,
                "content_index":0,"delta":"Synthetic partial answer"}),
            json!({"type":"response.failed","response":{"status":"failed","error":{
                "code":"subscription_sharing_usage_limit_exceeded",
                "message":"Synthetic quota rejection after output"
            }}}),
        ];
        crate::poison::mutex(&siwc.state.stream_prefixes).push_back(
            events
                .iter()
                .map(|event| format!("data: {event}\n\n"))
                .collect::<String>()
                .into_bytes(),
        );
        let runtime = runtime(&siwc, &next).await;
        let (base, task) = recovery::gateway(runtime).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"Synthetic input","stream":stream}))
            .send()
            .await
            .unwrap();
        let text = response.text().await.unwrap();
        assert!(
            text.contains("quota") || text.contains("usage_limit_exceeded"),
            "{text}"
        );
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
        assert_eq!(next.state.generation_requests.load(Ordering::SeqCst), 0);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_rejection_never_replays_repairs_or_changes_billing() {
    for (status, code) in [
        (
            StatusCode::UNAUTHORIZED,
            "subscription_sharing_invalid_user",
        ),
        (
            StatusCode::FORBIDDEN,
            "subscription_sharing_user_not_eligible",
        ),
        (StatusCode::TOO_MANY_REQUESTS, "rate_limit_exceeded"),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            "subscription_sharing_usage_unavailable",
        ),
        (StatusCode::BAD_REQUEST, "invalid_encrypted_content"),
    ] {
        let siwc = AccessServer::start(json!([]), false).await;
        let native = AccessServer::start(json!([]), false).await;
        *crate::poison::mutex(&siwc.state.generation_status) = status;
        *crate::poison::mutex(&siwc.state.generation_body) =
            json!({"error":{"code":code,"message":"Synthetic rejection"}});
        let runtime = runtime(&siwc, &native).await;
        runtime.set_route_recovery_enabled(true);
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"Synthetic input","stream":false}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(
            response.json::<Value>().await.unwrap()["error"]["code"],
            code
        );
        assert_eq!(
            siwc.state.generation_requests.load(Ordering::SeqCst),
            1,
            "{code}"
        );
        assert_eq!(
            native.state.generation_requests.load(Ordering::SeqCst),
            0,
            "{code}"
        );
        assert_eq!(
            runtime
                .lock_scheduler()
                .candidate("synthetic-account-0")
                .unwrap()
                .health,
            CandidateHealth::Healthy
        );
        task.abort();
    }
}

#[tokio::test]
async fn siwc_local_rejection_does_not_switch_to_native() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    let runtime = runtime(&siwc, &native).await;
    let (base, task) = recovery::gateway(runtime).await;
    let client = retry_admission::bounded_client();
    let response = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":"Synthetic input","temperature":0.3}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn siwc_admission_detail_survives_without_billing_fallback() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    let runtime = runtime(&siwc, &native).await;
    let (base, task) = recovery::gateway(runtime).await;
    let client = retry_admission::bounded_client();
    *crate::poison::mutex(&siwc.state.generation_status) = StatusCode::FORBIDDEN;
    *crate::poison::mutex(&siwc.state.generation_body) =
        json!({"detail":"Synthetic region restriction"});
    let response = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":"Synthetic input"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response.json::<Value>().await.unwrap(),
        json!({"detail":"Synthetic region restriction"})
    );
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}
