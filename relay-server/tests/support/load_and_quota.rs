use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn remote_gateway_serves_two_hundred_concurrent_requests_and_flushes_usage() {
    const REQUESTS: usize = 200;
    let root = TempDir::new().unwrap();
    let (upstream, load, upstream_task) = spawn_load_upstream(REQUESTS).await;
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let source: Value = client
        .post(format!("{}/sources", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "name": "Concurrent upstream",
            "baseUrl": format!("{upstream}/v1"),
            "apiKey": "synthetic-upstream-api-key",
            "wireApi": "responses",
            "models": ["gpt-test"]
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let source_id = source["id"].as_str().unwrap();
    assert_eq!(
        client
            .post(format!("{}/pool/members", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .json(&json!({"sourceIds": [source_id], "inPool": true}))
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let profile: Value = client
        .get(format!("{}/profile/credential", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let pool_key = profile["secret"].as_str().unwrap().to_string();

    let start = Arc::new(tokio::sync::Barrier::new(REQUESTS + 1));
    let tasks = (0..REQUESTS)
        .map(|index| {
            let client = client.clone();
            let origin = server.origin.clone();
            let pool_key = pool_key.clone();
            let start = start.clone();
            tokio::spawn(async move {
                start.wait().await;
                client
                    .post(format!("{origin}/v1/responses"))
                    .bearer_auth(pool_key)
                    .json(&json!({
                        "model": "gpt-test",
                        "input": format!("concurrent request {index}")
                    }))
                    .send()
                    .await
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    start.wait().await;
    let responses = tokio::time::timeout(Duration::from_secs(15), async {
        let mut responses = Vec::with_capacity(REQUESTS);
        for task in tasks {
            responses.push(task.await.unwrap());
        }
        responses
    })
    .await
    .expect("200 concurrent requests timed out");

    assert!(responses
        .iter()
        .all(|response| response.status() == StatusCode::OK));
    assert_eq!(load.total.load(Ordering::Relaxed), REQUESTS);
    assert_eq!(load.max_active.load(Ordering::Relaxed), REQUESTS);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let usage: Value = client
            .get(format!("{}/usage?page=1&pageSize=1", server.origin))
            .bearer_auth("synthetic-management-token-value")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        if usage["total"].as_u64() == Some(REQUESTS as u64) {
            break;
        }
        assert!(Instant::now() < deadline, "usage queue did not drain");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let persisted: Value = client
        .get(format!(
            "{}/usage?page=1&pageSize={REQUESTS}",
            server.origin
        ))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let events = persisted["events"].as_array().unwrap();
    assert_eq!(events.len(), REQUESTS);
    assert!(events.iter().any(|event| {
        event
            .pointer("/routing/inFlightBefore")
            .and_then(Value::as_u64)
            .is_some_and(|in_flight| in_flight > 0)
    }));
    let snapshot: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(!snapshot["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning == "usage_persistence_failed"));

    server.task.abort();
    upstream_task.abort();
}

#[tokio::test]
async fn adaptive_quota_refresh_has_no_remote_interval_setting() {
    let root = TempDir::new().unwrap();
    let server = spawn_server(root.path()).await;
    let client = reqwest::Client::new();

    let removed = client
        .post(format!("{}/quota/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({"refreshIntervalSeconds": 120, "requestTimeoutSeconds": 10}))
        .send()
        .await
        .unwrap();
    assert_eq!(removed.status(), StatusCode::NOT_FOUND);
    let state: Value = client
        .get(format!("{}/state", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(state["gateway"]
        .get("quotaRefreshIntervalSeconds")
        .is_none());
    assert!(state["gateway"].get("useFreeAccounts").is_none());

    let invalid_routing = client
        .post(format!("{}/routing/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "maxRetryCandidates": 0
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(invalid_routing.status(), StatusCode::BAD_REQUEST);

    let routing: Value = client
        .post(format!("{}/routing/settings", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .json(&json!({
            "maxRetryCandidates": 5,
            "routingStrategy": "subscription_plan",
            "subscriptionPlanOrder": ["not a valid\nplan"],
            "cooldownAfterFailures": 0,
            "keepLastCandidateAvailable": false,
            "imageBaseModel": "gpt-5.4-mini"
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(routing["gateway"]["maxRetryCandidates"], 5);
    assert!(routing["gateway"].get("sessionAffinity").is_none());
    assert!(routing["gateway"]
        .get("sessionAffinityTtlSeconds")
        .is_none());
    for old in [
        "cooldownAfterFailures",
        "keepLastCandidateAvailable",
        "routingStrategy",
        "subscriptionPlanOrder",
    ] {
        assert!(routing["gateway"].get(old).is_none());
    }
    assert_eq!(routing["gateway"]["imageBaseModel"], "gpt-5.4-mini");
    assert!(routing["capabilities"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|feature| feature == "runtime_routing"));
    let runtime_order: Value = client
        .get(format!("{}/routing/runtime", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(runtime_order.is_array());

    let refreshed: Value = client
        .post(format!("{}/pool/quota/refresh", server.origin))
        .bearer_auth("synthetic-management-token-value")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(refreshed["refreshed"], 0);
    assert_eq!(refreshed["failed"], 0);
    assert!(refreshed["snapshot"]["gateway"]
        .get("quotaRefreshIntervalSeconds")
        .is_none());
    assert!(refreshed["snapshot"]["gateway"]
        .get("useFreeAccounts")
        .is_none());
    assert_eq!(refreshed["snapshot"]["gateway"]["maxRetryCandidates"], 5);

    server.task.abort();
}
