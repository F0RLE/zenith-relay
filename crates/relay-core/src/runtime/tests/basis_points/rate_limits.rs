use super::*;

fn rate_limit() -> Value {
    json!({"error": {
        "code": "rate_limit_exceeded", "type": "tokens",
        "message": "Rate limit reached for gpt-test in organization org-synthetic on tokens per min (TPM): Limit 1000, Used 1000, Requested 100. Please try again in 2ms."
    }})
}

#[tokio::test]
async fn disabling_persistent_bps_rate_limit_wait_finishes_with_429_and_final_timing() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        *crate::poison::mutex(&server.state.generation_status) = StatusCode::TOO_MANY_REQUESTS;
        *crate::poison::mutex(&server.state.generation_body) = rate_limit();
        let events = Arc::new(std::sync::Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = retry_admission::pool_runtime(&server, 1, false).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        runtime.max_retry_candidates.store(8, Ordering::SeqCst);
        runtime.set_route_recovery_window_ms(1_000);
        runtime.set_route_recovery_enabled(true);
        let runtime = Arc::new(runtime);
        let (base, task) = recovery::gateway(runtime.clone()).await;
        let response = tokio::spawn(async move {
            retry_admission::bounded_client()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&json!({"model":"gpt-test","input":"synthetic input","stream":stream}))
                .send()
                .await
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            while crate::poison::mutex(&events).is_empty() {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(1_100)).await;
        assert!(!response.is_finished());
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        runtime.set_route_recovery_enabled(false);
        let response = tokio::time::timeout(Duration::from_secs(2), response)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key("retry-after"));
        let _ = response.bytes().await.unwrap();
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        let events = crate::poison::mutex(&events);
        assert_eq!(
            events
                .iter()
                .filter(|event| !event.is_request_completion())
                .count(),
            1
        );
        let final_event = events.last().unwrap();
        assert!(final_event.is_request_completion());
        assert_eq!(final_event.http_status, 429);
        let routing = final_event.routing.as_ref().unwrap();
        assert_eq!(
            routing.attempt_outcome,
            Some(crate::AttemptOutcome::ReturnedFailure)
        );
        assert!(routing.request_timing.as_ref().unwrap().duration_ms >= 900);
        assert!(routing.request_timing.as_ref().unwrap().duration_ms > final_event.latency_ms);
        drop(events);
        task.abort();
    }
}

#[tokio::test]
async fn retry_deadline_during_preparation_preserves_the_last_429_without_a_phantom_send() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        crate::poison::mutex(&server.state.generation_replies)
            .push_back((StatusCode::TOO_MANY_REQUESTS, rate_limit()));
        let events = Arc::new(std::sync::Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = retry_admission::pool_runtime(&server, 2, false).await;
        runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
        runtime.set_route_recovery_window_ms(1_000);
        runtime.set_route_recovery_enabled(false);
        let state = server.state.clone();
        let delayed = Arc::new(AtomicBool::new(false));
        let delayed_callback = delayed.clone();
        runtime.set_activity_callback(move |activity| {
            if activity.in_flight > 0
                && state.generation_requests.load(Ordering::SeqCst) > 0
                && !delayed_callback.swap(true, Ordering::SeqCst)
            {
                // Expire the budget after reserving the next candidate, before
                // preparation reaches the final outbound dispatch fence.
                std::thread::sleep(Duration::from_millis(1_100));
            }
        });
        let (base, task) = recovery::gateway(Arc::new(runtime)).await;
        let response = retry_admission::bounded_client()
            .post(format!("{base}/v1/responses"))
            .bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","input":"synthetic input","stream":stream}))
            .send()
            .await
            .unwrap();
        assert!(delayed.load(Ordering::SeqCst));
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(response.headers().contains_key("retry-after"));
        let _ = response.bytes().await.unwrap();
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        let events = crate::poison::mutex(&events);
        assert_eq!(events.len(), 2);
        assert!(!events[0].is_request_completion());
        assert!(events[1].is_request_completion());
        assert_eq!(events[1].http_status, 429);
        assert_eq!(
            events[1].error_category.as_deref(),
            Some(crate::error_codes::UPSTREAM_RATE_LIMITED)
        );
        assert!(
            events[1]
                .routing
                .as_ref()
                .unwrap()
                .request_timing
                .as_ref()
                .unwrap()
                .duration_ms
                >= 1_000
        );
        drop(events);
        task.abort();
    }
}

#[tokio::test]
async fn rate_limit_can_still_recover_on_an_independent_ready_account() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    crate::poison::mutex(&server.state.generation_replies)
        .push_back((StatusCode::TOO_MANY_REQUESTS, rate_limit()));
    let events = Arc::new(std::sync::Mutex::new(Vec::<crate::UsageEvent>::new()));
    let captured = events.clone();
    let mut runtime = retry_admission::pool_runtime(&server, 2, false).await;
    runtime.usage = Arc::new(move |event| crate::poison::mutex(&captured).push(event));
    runtime.set_route_recovery_enabled(true);
    runtime.set_route_recovery_window_ms(1_000);
    let (base, task) = recovery::gateway(Arc::new(runtime)).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":"synthetic input"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response.bytes().await.unwrap();
    let events = crate::poison::mutex(&events);
    assert_eq!(events.len(), 2);
    let last = events.last().unwrap();
    assert!(last.success);
    assert!(!last.is_request_completion());
    assert!(
        last.routing
            .as_ref()
            .unwrap()
            .request_timing
            .as_ref()
            .unwrap()
            .duration_ms
            >= last.latency_ms
    );
    drop(events);
    task.abort();
}

#[tokio::test]
async fn shared_bps_rate_limit_does_not_cycle_known_accounts_or_block_other_models() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}, {"id":"gpt-other"}]), false).await;
    crate::poison::mutex(&server.state.generation_replies).extend([
        (StatusCode::TOO_MANY_REQUESTS, rate_limit()),
        (StatusCode::TOO_MANY_REQUESTS, rate_limit()),
    ]);
    let runtime = Arc::new(retry_admission::pool_runtime(&server, 2, false).await);
    runtime.max_retry_candidates.store(2, Ordering::SeqCst);
    runtime.set_route_recovery_enabled(false);
    let (base, task) = recovery::gateway(runtime.clone()).await;
    let client = retry_admission::bounded_client();
    let before = runtime_now_ms();
    let response = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test", "input":"synthetic input"}))
        .send()
        .await
        .expect("the initial request must finish after both upstream rejections");
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
    // Admission may wait for the provider deadline, but must not dispatch meanwhile.
    let repeat = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test", "input":"synthetic input"}))
        .send();
    let _ = tokio::time::timeout(Duration::from_millis(100), repeat).await;
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
    {
        let scheduler = runtime.lock_scheduler();
        for member in ["synthetic-account-0", "synthetic-account-1"] {
            let candidate = scheduler.candidate(member).unwrap();
            assert!(candidate.cooldowns["gpt-test"] >= before + 5_000);
            assert!(!candidate.cooldowns.contains_key("*"));
            assert!(!candidate.cooldowns.contains_key("gpt-other"));
        }
    }
    let response = client
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-other", "input":"synthetic input"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 3);
    task.abort();
}

#[tokio::test]
async fn bps_allowance_limit_still_blocks_all_models() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}, {"id":"gpt-other"}]), false).await;
    crate::poison::mutex(&server.state.generation_replies).push_back((
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error":{"code":"usage_limit_reached", "resets_in_seconds":120}}),
    ));
    let runtime = Arc::new(runtime(&server).await);
    runtime.set_route_recovery_enabled(false);
    let (base, task) = recovery::gateway(runtime.clone()).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test", "input":"synthetic input"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(runtime
        .lock_scheduler()
        .candidate("account-1")
        .unwrap()
        .cooldowns
        .contains_key("*"));
    task.abort();
}
