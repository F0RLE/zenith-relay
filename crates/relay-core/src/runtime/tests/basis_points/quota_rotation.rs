use super::*;

#[tokio::test]
async fn bps_portable_tool_history_yields_to_fresh_quota_without_replaying_a_request() {
    for stream in [false, true] {
        for include_call in [true, false] {
            let server = AccessServer::start(json!([{"id":"gpt-test"}]), false).await;
            let runtime = Arc::new(super::retry_admission::pool_runtime(&server, 5, false).await);
            let now = crate::unix_time_ms();
            for (index, short, weekly) in [
                (0, 5_600, 4_600),
                (1, 4_200, 6_200),
                (2, 3_000, 6_000),
                (3, 2_700, 7_300),
                (4, 2_000, 7_200),
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
                Some(if include_call {
                    "synthetic-account-0"
                } else {
                    "synthetic-account-4"
                }),
                "stream={stream}, paired={include_call}"
            );
            assert_eq!(server.state.generation_requests.load(Ordering::SeqCst), 1);
            task.abort();
        }
    }
}
