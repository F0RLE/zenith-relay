use super::*;

#[tokio::test]
async fn generation_access_prioritizes_the_required_owner_before_slow_peers() {
    let fast = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let mut runtime = retry_admission::pool_runtime(&fast, 5, false).await;
    let key = runtime.authenticate_secret("local-secret").unwrap();
    let ordered = runtime
        .account_state
        .executors
        .values()
        .map(|account| account.id.clone())
        .collect::<Vec<_>>();
    let owner = ordered.last().unwrap().clone();
    let mut peers = Vec::new();
    for id in &ordered[..4] {
        let slow = AccessServer::start(json!([{"id":"gpt-test"}]), true).await;
        runtime
            .account_state
            .executors
            .get_mut(id)
            .unwrap()
            .basis_points_url = slow.responses_url.clone();
        peers.push(slow);
    }
    let runtime = Arc::new(runtime);
    tokio::time::timeout(
        Duration::from_secs(2),
        runtime.refresh_basis_points_access_for_request(
            &key,
            "gpt-test",
            &[WireApi::Responses],
            Some(&owner),
        ),
    )
    .await
    .expect("the required owner must not queue behind unrelated access reads");
    assert!(runtime.fresh_basis_points_access(&owner).is_some());
    assert_eq!(fast.state.generation_requests.load(Ordering::SeqCst), 0);
    for peer in &peers {
        peer.state.first_release.add_permits(1);
    }
}

#[tokio::test]
async fn generation_access_portable_history_does_not_wait_for_a_cooling_owner() {
    let fast = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let slow = AccessServer::start(json!([{"id":"gpt-test"}]), true).await;
    let mut runtime = retry_admission::pool_runtime(&fast, 2, false).await;
    runtime
        .account_state
        .executors
        .get_mut("synthetic-account-0")
        .unwrap()
        .basis_points_url = slow.responses_url.clone();
    runtime
        .prepare_basis_points_authorization("synthetic-account-1")
        .await
        .unwrap();
    let now = crate::unix_time_ms();
    runtime.bind_tool_call_affinity("key-1", "call_portable", "synthetic-account-0", now);
    runtime.lock_scheduler().set_cooldown_with_reason(
        "synthetic-account-0",
        "*",
        now + 60_000,
        crate::scheduler::CooldownReason::RateLimit,
    );
    let runtime = Arc::new(runtime);
    let (base, task) = super::recovery::gateway(runtime).await;
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test", "input":[
            {"type":"function_call","call_id":"call_portable","name":"lookup","arguments":"{}"},
            {"type":"function_call_output","call_id":"call_portable","output":"synthetic result"}
        ]}))
        .send()
        .await
        .expect("portable history must reach the ready peer without refreshing its old owner");
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    assert_eq!(fast.state.generation_requests.load(Ordering::SeqCst), 1);
    assert_eq!(slow.state.generation_requests.load(Ordering::SeqCst), 0);
    slow.state.first_release.add_permits(1);
    task.abort();
}

#[tokio::test]
async fn generation_access_does_not_wait_for_a_slow_peer() {
    for warm in [false, true] {
        let fast = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        let slow = AccessServer::start(json!([{"id":"gpt-test"}]), true).await;
        let mut runtime = retry_admission::pool_runtime(&fast, 2, false).await;
        runtime
            .account_state
            .executors
            .get_mut("synthetic-account-0")
            .unwrap()
            .basis_points_url = slow.responses_url.clone();
        if warm {
            runtime
                .prepare_basis_points_authorization("synthetic-account-1")
                .await
                .unwrap();
        }
        let runtime = Arc::new(runtime);
        let key = runtime.authenticate_secret("local-secret").unwrap();
        tokio::time::timeout(
            Duration::from_secs(2),
            runtime.refresh_basis_points_access_for_request(
                &key,
                "gpt-test",
                &[WireApi::Responses],
                None,
            ),
        )
        .await
        .expect("a ready peer must not wait for the blocked access read");
        assert!(
            runtime
                .fresh_basis_points_access("synthetic-account-1")
                .is_some()
        );
        assert!(
            runtime
                .fresh_basis_points_access("synthetic-account-0")
                .is_none()
        );
        tokio::time::timeout(Duration::from_secs(2), slow.state.first_started.notified())
            .await
            .unwrap();
        // Repeated warm requests coalesce background work and never grant the stale peer access.
        for _ in 0..3 {
            tokio::time::timeout(
                Duration::from_secs(2),
                runtime.refresh_basis_points_access_for_request(
                    &key,
                    "gpt-test",
                    &[WireApi::Responses],
                    None,
                ),
            )
            .await
            .unwrap();
        }
        assert_eq!(slow.state.access_requests.load(Ordering::SeqCst), 1);
        slow.state.first_release.add_permits(1);
        let _finished = tokio::time::timeout(
            Duration::from_secs(2),
            runtime.account_state.access_prefetch.lock(),
        )
        .await
        .unwrap();
        assert!(
            runtime
                .fresh_basis_points_access("synthetic-account-0")
                .is_some()
        );
        assert_eq!(fast.state.generation_requests.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn generation_access_does_not_reuse_expired_permissions_after_denial() {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let runtime = Arc::new(runtime(&server).await);
    runtime
        .prepare_basis_points_authorization("account-1")
        .await
        .unwrap();
    expire_access(&runtime);
    server.reply_with(StatusCode::FORBIDDEN, json!({"error":"synthetic denied"}));
    let key = runtime.authenticate_secret("local-secret").unwrap();
    runtime
        .refresh_basis_points_access_for_request(&key, "gpt-test", &[WireApi::Responses], None)
        .await;
    assert!(runtime.fresh_basis_points_access("account-1").is_none());
    assert!(runtime.basis_points_access_failed_for_model(&key, "gpt-test", &[WireApi::Responses]));
    runtime
        .refresh_basis_points_access_for_request(&key, "gpt-test", &[WireApi::Responses], None)
        .await;
    assert_eq!(server.state.access_requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn generation_access_waits_for_the_bound_response_owner() {
    let fast = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    let slow = AccessServer::start(json!([{"id":"gpt-test"}]), true).await;
    let mut runtime = retry_admission::pool_runtime(&fast, 2, false).await;
    runtime
        .account_state
        .executors
        .get_mut("synthetic-account-0")
        .unwrap()
        .basis_points_url = slow.responses_url.clone();
    runtime
        .prepare_basis_points_authorization("synthetic-account-1")
        .await
        .unwrap();
    let runtime = Arc::new(runtime);
    let key = runtime.authenticate_secret("local-secret").unwrap();
    let future = runtime.refresh_basis_points_access_for_request(
        &key,
        "gpt-test",
        &[WireApi::Responses],
        Some("synthetic-account-0"),
    );
    tokio::pin!(future);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut future)
            .await
            .is_err()
    );
    assert_eq!(slow.state.access_requests.load(Ordering::SeqCst), 1);
    assert!(
        runtime
            .fresh_basis_points_access("synthetic-account-0")
            .is_none()
    );
    slow.state.first_release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(2), future)
        .await
        .unwrap();
    assert!(
        runtime
            .fresh_basis_points_access("synthetic-account-0")
            .is_some()
    );
}
