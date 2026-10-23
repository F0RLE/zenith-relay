use super::*;

#[tokio::test]
async fn rate_limit_retry_after_cools_source_before_the_next_request() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![status_reply(
            StatusCode::TOO_MANY_REQUESTS,
            "limited",
            Some("60"),
        )],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![
            response_reply("resp-b-1", "ready"),
            response_reply("resp-b-2", "ready"),
        ],
    )
    .await;
    let (gateway, _) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn bounded_retry_does_not_report_an_untried_regular_source_as_cooled() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![status_reply(StatusCode::TOO_MANY_REQUESTS, "a", None)],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![status_reply(StatusCode::TOO_MANY_REQUESTS, "b", None)],
    )
    .await;
    let (source_c, state_c) =
        spawn_upstream("source-c-key", vec![response_reply("must-not-run", "c")]).await;
    let (gateway, _) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 20),
            source("source-b", &source_b, "source-b-key", &[MODEL], 10),
            source("source-c", &source_c, "source-c-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        2,
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "rate_limit_exceeded");
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 1);
    assert!(state_c.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn all_cooled_sources_keep_model_visible_and_return_local_retry_after() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![status_reply(
            StatusCode::TOO_MANY_REQUESTS,
            "a",
            Some("120"),
        )],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![status_reply(
            StatusCode::TOO_MANY_REQUESTS,
            "b",
            Some("120"),
        )],
    )
    .await;
    let (gateway, _) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    let first = request(&gateway, false).await;
    assert_eq!(first.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        first.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    assert_eq!(models(&gateway, LOCAL_KEY).await, [MODEL]);
    let before = (
        state_a.requests.lock().unwrap().len(),
        state_b.requests.lock().unwrap().len(),
    );

    let second = request(&gateway, false).await;
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        second.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    let body: Value = second.json().await.unwrap();
    assert_eq!(body["error"]["code"], "all_sources_cooling_down");
    assert_eq!(
        before,
        (
            state_a.requests.lock().unwrap().len(),
            state_b.requests.lock().unwrap().len(),
        )
    );
}

#[tokio::test]
async fn mixed_transient_and_rate_limit_cooldowns_return_service_unavailable() {
    let (source_a, state_a) =
        spawn_upstream("source-a-key", vec![overload_reply("a", Some("120"))]).await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![status_reply(
            StatusCode::TOO_MANY_REQUESTS,
            "b",
            Some("120"),
        )],
    )
    .await;
    let (gateway, _) = spawn_gateway_with_options(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            model_metadata_catalog: None,
            max_retry_candidates: 3,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let first = request(&gateway, false).await;
    assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        first.headers()["retry-after"]
            .to_str()
            .unwrap()
            .parse::<u64>()
            .unwrap()
            >= 1
    );
    let body: Value = first.json().await.unwrap();
    assert_eq!(body["error"]["code"], "all_sources_temporarily_unavailable");
    assert_eq!(models(&gateway, LOCAL_KEY).await, [MODEL]);
    let before = (
        state_a.requests.lock().unwrap().len(),
        state_b.requests.lock().unwrap().len(),
    );

    let second = request(&gateway, false).await;
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = second.json().await.unwrap();
    assert_eq!(body["error"]["code"], "all_sources_temporarily_unavailable");
    assert_eq!(
        before,
        (
            state_a.requests.lock().unwrap().len(),
            state_b.requests.lock().unwrap().len(),
        )
    );
}

#[tokio::test]
async fn known_invalid_prompt_is_terminal_and_does_not_call_the_fallback_source() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![Reply::Json {
            status: StatusCode::BAD_REQUEST,
            body: json!({"error": {"code": "invalid_prompt"}}),
            cache_control: "invalid-prompt",
            retry_after: None,
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![response_reply("must-not-run", "fallback")],
    )
    .await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        3,
    )
    .await;

    assert_eq!(
        request(&gateway, false).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert!(state_b.requests.lock().unwrap().is_empty());
    assert_eq!(events.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn overloaded_bad_request_falls_back_and_cools_only_the_model() {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![Reply::Json {
            status: StatusCode::BAD_REQUEST,
            body: json!({
                "error": {
                    "type": "invalid_request_error",
                    "code": "server_is_overloaded",
                    "message": "Please retry later."
                }
            }),
            cache_control: "overloaded",
            retry_after: None,
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![response_reply("fallback-response", "fallback")],
    )
    .await;
    let (gateway, events) = spawn_gateway_with_options(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            max_retry_candidates: 3,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "fallback-response"
    );
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 1);

    let events = events.lock().unwrap();
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_overloaded")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some(MODEL));
}

#[tokio::test]
async fn generic_provider_rejection_falls_back_and_cools_only_the_model() {
    assert_candidate_rejection_falls_back(json!({
        "code": "vendor_route_42",
        "message": "this route cannot serve the request"
    }))
    .await;
}

#[tokio::test]
async fn disabled_model_with_generic_request_error_still_falls_back() {
    assert_candidate_rejection_falls_back(json!({
        "type": "invalid_request_error",
        "code": "model_disabled",
        "message": "Requested model is disabled"
    }))
    .await;
}

async fn assert_candidate_rejection_falls_back(error: Value) {
    let (source_a, state_a) = spawn_upstream(
        "source-a-key",
        vec![Reply::Json {
            status: StatusCode::BAD_REQUEST,
            body: json!({"error": error}),
            cache_control: "rejected",
            retry_after: None,
        }],
    )
    .await;
    let (source_b, state_b) = spawn_upstream(
        "source-b-key",
        vec![
            response_reply("fallback-response-1", "fallback"),
            response_reply("fallback-response-2", "fallback"),
        ],
    )
    .await;
    let (gateway, events) = spawn_gateway_with_options(
        vec![
            source("source-a", &source_a, "source-a-key", &[MODEL], 10),
            source("source-b", &source_b, "source-b-key", &[MODEL], 0),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        GatewayRuntimeOptions {
            max_retry_candidates: 3,
            ..GatewayRuntimeOptions::default()
        },
    )
    .await;

    for expected_id in ["fallback-response-1", "fallback-response-2"] {
        let response = request(&gateway, false).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.json::<Value>().await.unwrap()["id"], expected_id);
    }

    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_candidate_rejected")
    );
    assert_eq!(events[0].cooldown_scope.as_deref(), Some(MODEL));
}

#[tokio::test]
async fn retry_budget_counts_execution_attempts_and_stops_before_third_source() {
    let (source_a, state_a) = spawn_upstream("a-key", vec![overload_reply("a", None)]).await;
    let (source_b, state_b) = spawn_upstream("b-key", vec![overload_reply("b", None)]).await;
    let (source_c, state_c) = spawn_upstream("c-key", vec![overload_reply("c", None)]).await;
    let (gateway, events) = spawn_gateway(
        vec![
            source("a", &source_a, "a-key", &[MODEL], 3),
            source("b", &source_b, "b-key", &[MODEL], 2),
            source("c", &source_c, "c-key", &[MODEL], 1),
        ],
        vec![local_key("key", LOCAL_KEY, None)],
        2,
    )
    .await;

    assert_eq!(
        request(&gateway, false).await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(state_a.requests.lock().unwrap().len(), 1);
    assert_eq!(state_b.requests.lock().unwrap().len(), 1);
    assert!(state_c.requests.lock().unwrap().is_empty());
    assert_eq!(events.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn exhausted_retry_preserves_a_safe_gateway_error_message() {
    let (upstream, state) = spawn_upstream(
        "gateway-key",
        vec![Reply::Json {
            status: StatusCode::SERVICE_UNAVAILABLE,
            body: json!({
                "error": {
                    "code": "service_unavailable",
                    "message": "no eligible source is available for this model"
                }
            }),
            cache_control: "gateway",
            retry_after: None,
        }],
    )
    .await;
    let (gateway, _) = spawn_gateway(
        vec![source("gateway", &upstream, "gateway-key", &[MODEL], 0)],
        vec![local_key("key", LOCAL_KEY, None)],
        1,
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["error"]["code"], "service_unavailable");
    assert_eq!(
        body["error"]["message"],
        "Provider: no eligible source is available for this model"
    );
    assert!(!body.to_string().contains(&upstream.base_url));
    assert_eq!(state.requests.lock().unwrap().len(), 1);
}
