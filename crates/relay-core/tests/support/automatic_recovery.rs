use super::*;
use zenith_relay_core::{resolve_pool_routing, PoolMemberKind, PoolRoutingMode};

fn options(ids: &[&str]) -> GatewayRuntimeOptions {
    let mut policy = resolve_pool_routing(
        None,
        ids.iter()
            .map(|id| (PoolMemberKind::Source, (*id).into(), 0, 1))
            .collect(),
    );
    policy.mode = PoolRoutingMode::InOrder;
    GatewayRuntimeOptions {
        pool_routing: Some(policy),
        // Existing saved settings must not truncate or disable recovery.
        max_retry_candidates: 1,
        cooldown_after_failures: 8,
        keep_last_candidate_available: true,
        ..Default::default()
    }
}

#[tokio::test]
async fn access_errors_and_opaque_rejections_do_not_hide_the_ninth_candidate() {
    let failures = [
        (StatusCode::UNAUTHORIZED, json!({"code":"invalid_api_key"})),
        (StatusCode::FORBIDDEN, json!({"code":"account_disabled"})),
        (StatusCode::FORBIDDEN, json!({"message":"access denied"})),
        (StatusCode::NOT_FOUND, json!({"code":"model_not_found"})),
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"code":"insufficient_quota"}),
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"code":"model_not_supported"}),
        ),
        (
            StatusCode::FORBIDDEN,
            json!({"code":"account_verification_required"}),
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"code":"invalid_request", "message":"Zenith AI request is invalid. Check the model, messages, tools, and parameters."}),
        ),
    ];
    let mut servers = Vec::new();
    let mut states = Vec::new();
    let mut sources = Vec::new();
    for (index, (status, error)) in failures.into_iter().enumerate() {
        let (server, state) = spawn_upstream(
            "test-key",
            vec![Reply::Json {
                status,
                body: json!({"error":error}),
                cache_control: "failure",
                retry_after: None,
            }],
        )
        .await;
        sources.push(source(
            &format!("source-{index}"),
            &server,
            "test-key",
            &[MODEL],
            0,
        ));
        servers.push(server);
        states.push(state);
    }
    let (server, success) =
        spawn_upstream("test-key", vec![response_reply("resp_ninth", "success")]).await;
    sources.push(source("source-8", &server, "test-key", &[MODEL], 0));
    let ids = sources
        .iter()
        .map(|source| source.source.id.as_str())
        .collect::<Vec<_>>();
    let policy = options(&ids);
    let (gateway, events) =
        spawn_gateway_with_options(sources, vec![local_key("key", LOCAL_KEY, None)], policy).await;
    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap()["id"], "resp_ninth");
    assert!(states
        .iter()
        .all(|state| state.requests.lock().unwrap().len() == 1));
    assert_eq!(success.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 9);
    assert!(events[..8]
        .iter()
        .all(|event| event.cooldown_scope.is_some()));
    assert!(events[8].success);
}

#[tokio::test]
async fn transient_failure_waits_five_seconds_and_recovers_without_manual_settings() {
    let (upstream, state) = spawn_upstream(
        "test-key",
        vec![
            status_reply(StatusCode::SERVICE_UNAVAILABLE, "failure", None),
            response_reply("resp_recovered", "success"),
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway_with_options(
        vec![source("source", &upstream, "test-key", &[MODEL], 0)],
        vec![local_key("key", LOCAL_KEY, None)],
        options(&["source"]),
    )
    .await;
    let started = std::time::Instant::now();
    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(started.elapsed() >= Duration::from_millis(4_950));
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "resp_recovered"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].consecutive_failures, Some(1));
    assert!(events[0].retry_at_ms.is_some());
    assert_eq!(events[1].consecutive_failures, Some(0));
}

#[tokio::test]
async fn repeated_failure_cools_the_last_candidate_and_stops_after_one_recovery() {
    let (upstream, state) = spawn_upstream(
        "test-key",
        vec![
            status_reply(StatusCode::SERVICE_UNAVAILABLE, "failure", None),
            status_reply(StatusCode::SERVICE_UNAVAILABLE, "failure", None),
            response_reply("must_not_run", "unexpected"),
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway_with_options(
        vec![source("source", &upstream, "test-key", &[MODEL], 0)],
        vec![local_key("key", LOCAL_KEY, None)],
        options(&["source"]),
    )
    .await;
    assert_eq!(
        request(&gateway, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[1].consecutive_failures, Some(2));
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(events[1].retry_at_ms.unwrap() >= now + 59_000);
}

#[tokio::test]
async fn mandatory_provider_delay_is_not_retried_within_the_recovery_window() {
    let (upstream, state) = spawn_upstream(
        "test-key",
        vec![
            status_reply(StatusCode::TOO_MANY_REQUESTS, "limited", Some("120")),
            response_reply("must_not_run", "unexpected"),
        ],
    )
    .await;
    let (gateway, _) = spawn_gateway_with_options(
        vec![source("source", &upstream, "test-key", &[MODEL], 0)],
        vec![local_key("key", LOCAL_KEY, None)],
        options(&["source"]),
    )
    .await;
    let response = tokio::time::timeout(Duration::from_secs(2), request(&gateway, false))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(state.requests.lock().unwrap().len(), 1);
}
