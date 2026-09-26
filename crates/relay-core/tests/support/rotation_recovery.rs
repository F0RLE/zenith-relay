use super::*;
use zenith_relay_core::{resolve_pool_routing, PoolMemberKind, PoolRoutingMode};

fn options(ids: &[&str]) -> GatewayRuntimeOptions {
    let mut policy = resolve_pool_routing(
        Some(&zenith_relay_core::PoolRoutingPolicy::default()),
        ids.iter()
            .map(|id| (PoolMemberKind::Source, (*id).into(), 0, 1))
            .collect(),
    );
    policy.mode = PoolRoutingMode::InOrder;
    GatewayRuntimeOptions {
        pool_routing: Some(policy),
        // Every recovery and compatibility dispatch shares this exact limit.
        max_retry_candidates: 3,
        ..Default::default()
    }
}

#[tokio::test]
async fn bounded_requests_reach_the_ninth_candidate_without_hiding_it_from_the_pool() {
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
    // A single request may dispatch only three times, even if the pool has
    // more routes. Independent requests can continue past the cooled slots.
    assert_ne!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(
        states
            .iter()
            .map(|state| state.requests.lock().unwrap().len())
            .sum::<usize>(),
        3
    );
    assert!(success.requests.lock().unwrap().is_empty());
    assert_ne!(request(&gateway, false).await.status(), StatusCode::OK);
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
async fn transient_failure_uses_rotation_pacing_and_recovers_without_manual_refresh() {
    let (upstream, state) = spawn_upstream(
        "test-key",
        vec![
            overload_reply("failure", None),
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
    assert!(started.elapsed() >= Duration::from_millis(240));
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
async fn repeated_rejection_stops_at_the_shared_request_budget_without_multiple_health_votes() {
    let (upstream, state) = spawn_upstream(
        "test-key",
        vec![
            overload_reply("failure", None),
            overload_reply("failure", None),
            overload_reply("failure", None),
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
    assert_eq!(state.requests.lock().unwrap().len(), 3);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert!(events
        .iter()
        .all(|event| event.consecutive_failures == Some(1)));
    for (index, event) in events.iter().enumerate() {
        assert_eq!(usize::from(event.attempt), index + 1);
        assert_eq!(event.request_id, events[0].request_id);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    assert!(events[2].retry_at_ms.unwrap() >= now + 150);
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
