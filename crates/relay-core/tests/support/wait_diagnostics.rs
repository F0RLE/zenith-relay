use super::*;

#[tokio::test]
async fn retry_after_wait_is_measured_without_shortening_the_provider_deadline() {
    let (upstream, state) = spawn_upstream(
        "synthetic-key",
        vec![
            status_reply(StatusCode::TOO_MANY_REQUESTS, "limited", Some("1")),
            response_reply("recovered", "ready"),
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway_with_options(
        vec![source("source-a", &upstream, "synthetic-key", &[MODEL], 0)],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            diagnostics_enabled: || true,
            max_retry_candidates: 3,
            ..Default::default()
        },
    )
    .await;
    let started = std::time::Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(5), request(&gateway, false))
        .await
        .expect("synthetic recovery must stay bounded");
    assert_eq!(response.status(), StatusCode::OK);
    let elapsed_ms = started.elapsed().as_millis();
    assert!(elapsed_ms >= 1_000, "Retry-After must not be shortened");
    assert_eq!(state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    let attempts = usage_observations::physical_attempts(&events);
    assert_eq!(attempts.len(), 2);
    assert!(!attempts[0].success);
    assert!(attempts[1].success);
    let encoded = serde_json::to_value(attempts[1]).unwrap();
    let timing = &encoded["routing"]["requestTiming"];
    eprintln!(
        "retry-after fixture: elapsed_ms={elapsed_ms}, attempts={}, wait_diagnostics={}",
        attempts.len(),
        timing["waiting"]
    );
    let waiting = &timing["waiting"];
    let measured = waiting["recovery"]["durationMs"]
        .as_u64()
        .expect("recovery waiting must be observable independently of generation");
    assert!(
        measured >= 990,
        "the scheduled cooldown accounts for the wait"
    );
    assert!(measured <= timing["durationMs"].as_u64().unwrap());
    assert_eq!(waiting["recovery"]["count"], 1);
    assert!(waiting["capacity"].is_null());
    assert!(waiting["poolEvent"].is_null());
    assert!(waiting["stopReason"].is_null());
    let delay = waiting["lastRecoveryDelayMs"].as_u64().unwrap();
    assert!((990..=1_000).contains(&delay));
    assert!(attempts[1].latency_ms < timing["durationMs"].as_u64().unwrap());
}

#[tokio::test]
async fn healthy_fallback_does_not_wait_for_another_members_retry_after() {
    let (a, state_a) = spawn_upstream(
        "synthetic-a",
        vec![status_reply(
            StatusCode::TOO_MANY_REQUESTS,
            "limited",
            Some("60"),
        )],
    )
    .await;
    let (b, state_b) =
        spawn_upstream("synthetic-b", vec![response_reply("fallback", "ready")]).await;
    let (gateway, events) = spawn_gateway_with_options(
        vec![
            source("a", &a, "synthetic-a", &[MODEL], 10),
            source("b", &b, "synthetic-b", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            diagnostics_enabled: || true,
            max_retry_candidates: 3,
            ..Default::default()
        },
    )
    .await;
    let started = std::time::Instant::now();
    let response = tokio::time::timeout(Duration::from_secs(3), request(&gateway, false))
        .await
        .expect("healthy fallback must not inherit another member's cooldown");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    let attempts = usage_observations::physical_attempts(&events);
    assert_eq!(attempts.len(), 2);
    let encoded = serde_json::to_value(attempts[1]).unwrap();
    assert!(encoded["routing"]["requestTiming"]["waiting"].is_null());
    eprintln!(
        "healthy fallback fixture: elapsed_ms={}, attempts={}, wait_diagnostics=null",
        started.elapsed().as_millis(),
        attempts.len()
    );
}
