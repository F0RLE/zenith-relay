use super::*;

fn rate_limit() -> Value {
    json!({"error": {
        "code": "rate_limit_exceeded", "type": "tokens",
        "message": "Rate limit reached for gpt-test in organization org-synthetic on tokens per min (TPM): Limit 1000, Used 1000, Requested 100. Please try again in 2ms."
    }})
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
