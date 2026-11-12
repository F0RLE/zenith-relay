use super::*;

#[tokio::test]
async fn siwc_output_limit_is_omitted_by_default_for_client_formats_with_diagnostics() {
    for stream in [false, true] {
        for (endpoint, input) in [
            (
                "responses",
                json!({"input":"Synthetic input","max_output_tokens":1024}),
            ),
            (
                "chat/completions",
                json!({"messages":[{"role":"user","content":"Synthetic input"}],"max_completion_tokens":1024}),
            ),
            (
                "chat/completions",
                json!({"messages":[{"role":"user","content":"Synthetic input"}],"max_tokens":1024}),
            ),
            (
                "messages",
                json!({"messages":[{"role":"user","content":"Synthetic input"}],"max_tokens":1024}),
            ),
        ] {
            let server = AccessServer::start(json!([]), false).await;
            server
                .state
                .stream_generations
                .store(true, Ordering::SeqCst);
            *crate::poison::mutex(&server.state.generation_body) = json::completed_response();
            let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
            let captured = events.clone();
            let finished = Arc::new(Notify::new());
            let notified = finished.clone();
            let native = AccessServer::start(json!([]), false).await;
            let mut runtime = runtime(&server, &native).await;
            {
                let mut scheduler = runtime.lock_scheduler();
                let mut excluded = scheduler.candidate("synthetic-account-1").unwrap().clone();
                excluded.model_rules.excluded.insert("gpt-test".into());
                scheduler.upsert(excluded);
            }
            let settings = Arc::get_mut(&mut runtime).unwrap();
            settings.diagnostics_enabled = || true;
            settings.usage = Arc::new(move |event| {
                crate::poison::mutex(&captured).push(event);
                notified.notify_one();
            });
            let (base, task) = recovery::gateway(runtime.clone()).await;
            let mut request = input;
            request["model"] = json!("gpt-test");
            request["stream"] = json!(stream);
            let client = retry_admission::bounded_client();
            let response = client
                .post(format!("{base}/v1/{endpoint}"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{endpoint} stream={stream}: {text}");
            assert!(text.contains("Synthetic JSON answer"), "{text}");
            tokio::time::timeout(Duration::from_secs(3), finished.notified())
                .await
                .unwrap();
            let bodies = crate::poison::mutex(&server.state.websocket_bodies);
            assert_eq!(bodies.len(), 1);
            assert!(bodies[0].get("max_output_tokens").is_none());
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 0);
            assert_eq!(
                server.state.http_generation_requests.load(Ordering::SeqCst),
                0
            );
            let events = crate::poison::mutex(&events);
            let success = events.iter().find(|event| event.success).unwrap();
            assert_eq!(
                success
                    .routing
                    .as_ref()
                    .unwrap()
                    .transport
                    .as_ref()
                    .unwrap()
                    .request_preparation
                    .as_ref()
                    .unwrap()
                    .siwc_output_limit_omitted,
                Some(true)
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
}

#[tokio::test]
async fn siwc_output_limit_omission_does_not_remove_native_account_limits() {
    let server = AccessServer::start(json!([]), false).await;
    server
        .state
        .stream_generations
        .store(true, Ordering::SeqCst);
    *crate::poison::mutex(&server.state.generation_body) = json::completed_response();
    let siwc = AccessServer::start(json!([]), false).await;
    let runtime = runtime(&siwc, &server).await;
    {
        let mut scheduler = runtime.lock_scheduler();
        let mut excluded = scheduler.candidate("synthetic-account-0").unwrap().clone();
        excluded.model_rules.excluded.insert("gpt-test".into());
        scheduler.upsert(excluded);
    }
    let (base, task) = recovery::gateway(runtime).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":"Synthetic input","max_output_tokens":1024}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{text}");
    let bodies = crate::poison::mutex(&server.state.generation_bodies);
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0]["max_output_tokens"], 1024);
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
    task.abort();
}

#[tokio::test]
async fn siwc_unsupported_controls_select_a_compatible_route_preserving_output_limits() {
    for stream in [false, true] {
        for (endpoint, request) in [
            (
                "responses",
                json!({"input":"Synthetic input","max_output_tokens":1024}),
            ),
            (
                "chat/completions",
                json!({"messages":[{"role":"user","content":"Synthetic input"}],
                    "max_completion_tokens":1024}),
            ),
            (
                "messages",
                json!({"messages":[{"role":"user","content":"Synthetic input"}],
                    "max_tokens":1024}),
            ),
        ] {
            let siwc = AccessServer::start(json!([]), false).await;
            let native = AccessServer::start(json!([]), false).await;
            native
                .state
                .stream_generations
                .store(true, Ordering::SeqCst);
            *crate::poison::mutex(&native.state.generation_body) = json::completed_response();
            let runtime = runtime(&siwc, &native).await;
            let (base, task) = recovery::gateway(runtime.clone()).await;
            let mut request = request;
            request["model"] = json!("gpt-test");
            request["stream"] = json!(stream);
            request["temperature"] = json!(0.3);
            let original = request.clone();
            let response = retry_admission::bounded_client()
                .post(format!("{base}/v1/{endpoint}"))
                .bearer_auth("local-secret")
                .json(&request)
                .send()
                .await
                .unwrap();
            let status = response.status();
            let text = response.text().await.unwrap();
            assert_eq!(status, StatusCode::OK, "{endpoint} stream={stream}: {text}");
            assert!(text.contains("Synthetic JSON answer"), "{text}");
            assert_eq!(request, original);
            assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
            assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 1);
            let bodies = crate::poison::mutex(&native.state.generation_bodies);
            assert_eq!(bodies.len(), 1);
            assert_eq!(bodies[0]["max_output_tokens"], 1024);
            assert_eq!(bodies[0]["temperature"], 0.3);
            drop(bodies);
            assert_eq!(
                runtime
                    .lock_scheduler()
                    .candidate("synthetic-account-0")
                    .unwrap()
                    .health,
                CandidateHealth::Healthy
            );
            assert!(
                runtime
                    .candidate_runtime_order()
                    .iter()
                    .all(|candidate| candidate.in_flight == 0)
            );
            task.abort();
        }
    }
}

#[tokio::test]
async fn siwc_invalid_output_limit_never_sends_or_changes_health() {
    for recovery_enabled in [false, true] {
        let siwc = AccessServer::start(json!([]), false).await;
        let usage = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = usage.clone();
        let mut runtime = retry_admission::pool_runtime(&siwc, 1, false).await;
        runtime
            .account_state
            .executors
            .get_mut("synthetic-account-0")
            .unwrap()
            .oauth_client_kind = OAuthClientKind::Siwc;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        let runtime = Arc::new(runtime);
        runtime.set_route_recovery_enabled(recovery_enabled);
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"Synthetic input",
                "max_output_tokens":0,"stream":false}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let error: Value = response.json().await.unwrap();
        assert_eq!(error["error"]["code"], "adapter_invalid_request");
        assert_eq!(error["error"]["param"], "max_output_tokens");
        assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
        assert_eq!(siwc.state.access_requests.load(Ordering::SeqCst), 0);
        assert_eq!(
            runtime
                .lock_scheduler()
                .candidate("synthetic-account-0")
                .unwrap()
                .health,
            CandidateHealth::Healthy
        );
        let usage = crate::poison::mutex(&usage);
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].attempt, 0);
        assert!(usage[0].account_id.is_none());
        assert!(usage[0].candidate_id.is_none());
        assert!(usage[0].total_tokens.is_none());
        drop(usage);
        task.abort();
    }
}

#[tokio::test]
async fn siwc_unsupported_control_preserves_owned_replay_on_a_compatible_route() {
    let siwc = AccessServer::start(json!([]), false).await;
    let native = AccessServer::start(json!([]), false).await;
    let runtime = runtime(&siwc, &native).await;
    let now = crate::unix_time_ms();
    let route = runtime
        .executor_route(
            "synthetic-account-0",
            "gpt-test",
            &Default::default(),
            &[crate::WireApi::Responses],
            false,
        )
        .unwrap();
    runtime.bind_response_route_affinity(Some("resp_synthetic_owned"), &route, now);
    runtime.capture_native_responses_replay(
        "key-1",
        "synthetic-account-0",
        &json!({"input":[{"role":"user","content":"Synthetic first turn"}]}),
        "gpt-test",
        &json!({"id":"resp_synthetic_owned","status":"completed","output":[]}),
        now,
    );
    let (base, task) = recovery::gateway(runtime).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":"Synthetic next turn",
            "previous_response_id":"resp_synthetic_owned","max_output_tokens":1024,
            "temperature":0.3}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 0);
    assert_eq!(native.state.generation_requests.load(Ordering::SeqCst), 1);
    let bodies = crate::poison::mutex(&native.state.generation_bodies);
    assert_eq!(bodies[0]["max_output_tokens"], 1024);
    assert_eq!(bodies[0]["temperature"], 0.3);
    assert!(bodies[0].get("previous_response_id").is_none());
    assert_eq!(
        bodies[0]["input"],
        json!([
            {"role":"user","content":"Synthetic first turn"},
            {"role":"user","content":[{"type":"input_text","text":"Synthetic next turn"}]}
        ])
    );
    task.abort();
}
