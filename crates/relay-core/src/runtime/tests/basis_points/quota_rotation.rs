use super::*;

#[tokio::test]
async fn bps_quota_wait_keeps_the_owner_when_other_routes_reject_opaque_history() {
    for stream in [false, true] {
        quota_wait_with_opaque_history(stream).await;
    }
}

async fn quota_wait_with_opaque_history(stream: bool) {
    let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
    *crate::poison::mutex(&server.state.generation_body) = json!({
        "id":"resp_owner","model":"gpt-test","status":"completed","output":[
            {"type":"reasoning","id":"rs_owner","summary":[],"encrypted_content":"synthetic-cipher"},
            {"type":"function_call","id":"fc_lookup","call_id":"call_lookup","name":"run_officejs",
                "arguments":json!({"references":["lookup"],"code":"{\"path\":\"synthetic\"}"}).to_string(),
                "status":"completed"}
        ]
    });
    let rejected = Arc::new(Notify::new());
    let observed = rejected.clone();
    let mut runtime = super::retry_admission::pool_runtime(&server, 2, false).await;
    runtime.usage = Arc::new(move |event| {
        if event.error_category.as_deref() == Some("upstream_quota_exhausted") {
            observed.notify_one();
        }
    });
    let runtime = Arc::new(runtime);
    runtime.set_route_recovery_enabled(true);
    let (base, gateway) = super::recovery::gateway(runtime.clone()).await;
    let client = reqwest::Client::new();
    let tools = json!([{"name":"lookup","input_schema":{"type":"object",
        "properties":{"path":{"type":"string"}},"required":["path"]}}]);
    let first = client
        .post(format!("{base}/v1/messages"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","max_tokens":64,"tools":tools,
            "messages":[{"role":"user","content":"Look up the synthetic path"}]}))
        .send()
        .await
        .unwrap();
    let status = first.status();
    let body: Value = first.json().await.unwrap();
    assert_eq!(status, StatusCode::OK, "{body}");
    let tool_id = body["content"]
        .as_array()
        .unwrap()
        .iter()
        .find(|block| block["type"] == "tool_use")
        .unwrap()["id"]
        .clone();
    crate::poison::mutex(&server.state.generation_replies).push_back((
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error":{"type":"usage_limit_reached","resets_in_seconds":1360}}),
    ));
    let request = tokio::spawn(async move {
        client.post(format!("{base}/v1/messages")).bearer_auth("local-secret")
            .json(&json!({"model":"gpt-test","max_tokens":64,"stream":stream,"tools":tools,"messages":[
                {"role":"user","content":"Look up the synthetic path"},
                {"role":"assistant","content":body["content"]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":tool_id,"content":"Synthetic result"}]}
            ]})).send().await.unwrap()
    });
    tokio::time::timeout(Duration::from_secs(3), rejected.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        !request.is_finished(),
        "an unrelated incompatible route disabled quota recovery"
    );
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
    runtime.set_route_recovery_enabled(false);
    let response = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let _ = response.bytes().await.unwrap();
    assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
    gateway.abort();
}

#[tokio::test]
async fn bps_quota_rejection_waits_for_a_newly_available_route() {
    for stream in [false, true] {
        let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
        crate::poison::mutex(&server.state.generation_replies).push_back((
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"type":"usage_limit_reached",
                "message":"The usage limit has been reached","resets_in_seconds":1360}}),
        ));
        let rejected = Arc::new(Notify::new());
        let observed = rejected.clone();
        let events = Arc::new(Mutex::new(Vec::<crate::UsageEvent>::new()));
        let captured = events.clone();
        let mut runtime = super::retry_admission::pool_runtime(&server, 2, false).await;
        runtime.usage = Arc::new(move |event| {
            let quota = event.error_category.as_deref() == Some("upstream_quota_exhausted");
            crate::poison::mutex(&captured).push(event);
            if quota {
                observed.notify_one();
            }
        });
        let runtime = Arc::new(runtime);
        runtime.set_route_recovery_enabled(true);
        assert!(runtime.sync_account_availability_with_quota(
            "synthetic-account-1",
            false,
            CandidateHealth::Healthy,
            &QuotaSnapshot::default(),
            crate::unix_time_ms(),
        ));
        let (base, gateway) = super::recovery::gateway(runtime.clone()).await;
        let request = tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("{base}/v1/messages"))
                .bearer_auth("local-secret")
                .json(&json!({"model":"gpt-test","stream":stream,"max_tokens":64,
                    "messages":[{"role":"user","content":"Synthetic question"}]}))
                .send()
                .await
                .unwrap()
        });
        tokio::time::timeout(Duration::from_secs(3), rejected.notified())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !request.is_finished(),
            "quota rejection bypassed persistent route recovery"
        );
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
        assert!(runtime.sync_account_availability_with_quota(
            "synthetic-account-1",
            true,
            CandidateHealth::Healthy,
            &QuotaSnapshot::default(),
            crate::unix_time_ms(),
        ));
        let response = tokio::time::timeout(Duration::from_secs(3), request)
            .await
            .unwrap()
            .unwrap();
        let status = response.status();
        let body = response.text().await.unwrap();
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 2);
        gateway.abort();
    }
}

#[tokio::test]
async fn bps_portable_tool_history_preserves_quota_affinity_without_replay() {
    for stream in [false, true] {
        for (include_call, owner_short) in
            [(true, 2_000), (false, 2_000), (true, 1_000), (false, 1_000)]
        {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            let runtime = Arc::new(super::retry_admission::pool_runtime(&server, 5, false).await);
            let now = crate::unix_time_ms();
            for (index, short, weekly) in [
                (0, 5_600, 4_600),
                (1, 4_200, 6_200),
                (2, 3_000, 6_000),
                (3, 2_700, 7_300),
                (4, owner_short, 7_200),
            ] {
                let window = |kind, remaining, minutes| crate::quota::QuotaWindow {
                    kind,
                    provider_cycle_id: None,
                    window_start_ms: None,
                    available_basis_points: Some(remaining),
                    explicitly_full: None,
                    reset_at_ms: Some(now + u64::from(minutes) * 60_000),
                    window_minutes: Some(minutes),
                    observed_at_ms: now,
                    full_transition_fingerprint: None,
                    exhaustion_transition_fingerprint: None,
                };
                let quota = QuotaSnapshot {
                    primary: Some(window(crate::quota::QuotaWindowKind::Primary, short, 300)),
                    secondary: Some(window(
                        crate::quota::QuotaWindowKind::Secondary,
                        weekly,
                        10_080,
                    )),
                    updated_at_ms: Some(now),
                    ..Default::default()
                };
                assert!(runtime.sync_account_availability_with_quota(
                    &format!("synthetic-account-{index}"),
                    true,
                    CandidateHealth::Healthy,
                    &quota,
                    now,
                ));
            }
            runtime.bind_tool_call_affinity("key-1", "call_quota", "synthetic-account-4", now);
            let mut input = Vec::new();
            if include_call {
                input.push(json!({
                    "type":"function_call", "call_id":"call_quota", "name":"lookup", "arguments":"{}"
                }));
            }
            input.push(json!({
                "type":"function_call_output", "call_id":"call_quota", "output":"synthetic result"
            }));
            let (base, task) = super::recovery::gateway(runtime.clone()).await;
            let response = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap()
                .post(format!("{base}/v1/responses"))
                .bearer_auth("local-secret")
                .json(&json!({"model":"gpt-test", "stream":stream, "input":input}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let _ = response.bytes().await.unwrap();
            let affinity = runtime
                .response_affinity_key(Some("synthetic-response"))
                .unwrap();
            assert_eq!(
                runtime
                    .response_affinity_candidate(&affinity, crate::unix_time_ms())
                    .as_deref(),
                Some(if include_call && owner_short <= 1_000 {
                    "synthetic-account-0"
                } else {
                    "synthetic-account-4"
                }),
                "stream={stream}, paired={include_call}, owner_short={owner_short}"
            );
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
            task.abort();
        }
    }
}
