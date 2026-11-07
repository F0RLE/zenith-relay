use super::*;

#[tokio::test]
async fn siwc_quota_without_structured_proof_never_replays() {
    for client_stream in [false, true] {
        for upstream_stream in [false, true] {
            for error in [
                json!({"type":"invalid_request_error","message":
                    "The ChatGPT user has reached their Subscription Sharing usage limit."}),
                json!({"message":"subscription_sharing_usage_limit_exceeded"}),
                json!({"code":"insufficient_quota","message":"Synthetic quota refusal"}),
                json!({"type":"usage_limit_reached","message":"Synthetic native quota refusal"}),
                json!({"code":null,"type":"subscription_sharing_usage_limit_exceeded"}),
                json!({"code":"provider_unknown","type":"subscription_sharing_usage_limit_exceeded"}),
            ] {
                assert_no_replay(&error.to_string(), client_stream, upstream_stream).await;
            }
        }
    }
}

#[tokio::test]
async fn siwc_duplicate_sse_discriminators_never_replay() {
    for client_stream in [false, true] {
        for error in [
            r#"{"code":"provider_unknown","code":"subscription_sharing_usage_limit_exceeded"}"#,
            r#"{"code":"provider_unknown","co\u0064e":"subscription_sharing_usage_limit_exceeded"}"#,
        ] {
            assert_no_replay(error, client_stream, true).await;
        }
    }
}

async fn assert_no_replay(error: &str, client_stream: bool, upstream_stream: bool) {
    let siwc = AccessServer::start(json!([]), false).await;
    let next = AccessServer::start(json!([]), false).await;
    if upstream_stream {
        siwc.state.stream_generations.store(true, Ordering::SeqCst);
        // Preserve raw duplicate fields through the upstream wire, not a Value.
        crate::poison::mutex(&siwc.state.stream_prefixes).push_back(
            format!(
                "data: {{\"type\":\"response.failed\",\"response\":{{\"id\":\"synthetic-quota-refusal\",\"status\":\"failed\",\"error\":{error}}}}}\n\n"
            )
            .into_bytes(),
        );
    } else {
        *crate::poison::mutex(&siwc.state.generation_status) = StatusCode::TOO_MANY_REQUESTS;
        *crate::poison::mutex(&siwc.state.generation_body) =
            json!({"error":serde_json::from_str::<Value>(error).unwrap()});
    }
    let mut runtime = runtime(&siwc, &next).await;
    Arc::get_mut(&mut runtime)
        .unwrap()
        .account_state
        .executors
        .get_mut("synthetic-account-1")
        .unwrap()
        .oauth_client_kind = OAuthClientKind::Siwc;
    runtime.set_route_recovery_enabled(true);
    let (base, task) = recovery::gateway(runtime.clone()).await;
    let response = retry_admission::bounded_client()
        .post(format!("{base}/v1/responses"))
        .bearer_auth("local-secret")
        .json(&json!({"model":"gpt-test","input":"Synthetic input","stream":client_stream}))
        .send()
        .await
        .unwrap();
    let status = response.status();
    let body = response.text().await.unwrap();
    assert!(!status.is_success(), "{error}: {body}");
    assert_eq!(siwc.state.generation_requests.load(Ordering::SeqCst), 1);
    assert_eq!(next.state.generation_requests.load(Ordering::SeqCst), 0);
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
