use super::*;

#[tokio::test]
async fn image_errors_retry_only_proven_rejections_without_generation() {
    let cases = [
        (
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"token_expired"},"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}),
            false,
        ),
        (
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"token_expired"},"data":[{"b64_json":"aW1hZ2U="}]}),
            false,
        ),
        (
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"token_expired"},"svg_usage":{"model":"synthetic-author","input_tokens":2,"output_tokens":3,"total_tokens":5}}),
            false,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"code":"rate_limit_exceeded"},"usage":{"input_tokens":2,"output_tokens":3,"total_tokens":5}}),
            false,
        ),
        (
            StatusCode::FORBIDDEN,
            json!({"error":{"code":"image_generation_not_enabled"},"data":[{"b64_json":"aW1hZ2U="}]}),
            false,
        ),
        (
            StatusCode::FORBIDDEN,
            json!({"error":{"code":"image_generation_not_enabled"},"data":[{"url":"https://example.invalid/image"}]}),
            false,
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"error":{"message":"Image generation is not enabled"},"svg_usage":{"model":"synthetic-author","input_tokens":2,"output_tokens":3,"total_tokens":5}}),
            false,
        ),
        (
            StatusCode::BAD_REQUEST,
            json!({"error":{"type":"invalid_request_error","message":"Invalid size"},"prompt":"image generation is not enabled"}),
            false,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"code":"rate_limit_exceeded"},"data":42,"svg_usage":{"output_tokens":3}}),
            false,
        ),
        (
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error":{"code":"rate_limit_exceeded"}}),
            true,
        ),
        (
            StatusCode::FORBIDDEN,
            json!({"error":{"code":"image_generation_not_enabled"}}),
            true,
        ),
    ];
    for native_account in [false, true] {
        for (status, body, retry) in &cases {
            let (first, first_state) =
                spawn_upstream(vec![Reply::Json(*status, body.clone())]).await;
            let (fallback, fallback_state) = spawn_upstream(vec![Reply::Json(
                StatusCode::OK,
                json!({"data":[{"b64_json":"ZmFsbGJhY2s="}]}),
            )])
            .await;
            let mut fallback_source = source("image-fallback", &fallback, "synthetic-fallback", 0);
            fallback_source.source.models = vec![zenith_relay_core::IMAGE_API_MODEL.into()];
            let mut sources = vec![fallback_source];
            let accounts = if native_account {
                vec![account("image-account", "synthetic-provider", &first, 10)]
            } else {
                let mut first_source = source("image-first", &first, "synthetic-first", 10);
                first_source.source.models = vec![zenith_relay_core::IMAGE_API_MODEL.into()];
                sources.push(first_source);
                Vec::new()
            };
            let (gateway, events, _, _) = spawn_mixed_gateway_with_options(
                sources,
                accounts,
                vec![mixed_key(None, None)],
                ready_authority("image-account", "synthetic-access").await,
                refresh_adapter(),
                Arc::new(PersistenceAdapter::default()),
                GatewayRuntimeOptions {
                    pool_routing: Some(zenith_relay_core::PoolRoutingPolicy {
                        mode: zenith_relay_core::PoolRoutingMode::Failover,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            )
            .await;
            let response = reqwest::Client::new()
                .post(format!("{}/v1/images/generations", gateway.base_url))
                .bearer_auth(LOCAL_KEY)
                .json(&json!({"model":zenith_relay_core::IMAGE_API_MODEL,"prompt":"synthetic"}))
                .send()
                .await
                .unwrap();
            assert_eq!(
                response.status(),
                if *retry { StatusCode::OK } else { *status },
                "native={native_account}, {body}"
            );
            let _: Value = response.json().await.unwrap();
            assert_eq!(first_state.requests.lock().unwrap().len(), 1, "{body}");
            assert_eq!(
                fallback_state.requests.lock().unwrap().len(),
                usize::from(*retry),
                "{body}"
            );
            let events = events.lock().unwrap();
            assert_eq!(events.len(), 1 + usize::from(*retry), "{body}");
            assert!(!events[0].success);
            if body.get("data").is_some_and(Value::is_array) {
                assert_eq!(events[0].image_usage.as_ref().unwrap().generated_images, 1);
            }
            if let Some(count) = body.pointer("/usage/output_tokens").and_then(Value::as_u64) {
                assert_eq!(events[0].output_tokens, Some(count));
            }
            if body.get("data").is_none() && body.get("svg_usage").is_some() {
                assert_eq!(
                    events[0]
                        .image_usage
                        .as_ref()
                        .unwrap()
                        .svg_usage
                        .as_ref()
                        .unwrap()
                        .output_tokens,
                    Some(3)
                );
            }
        }
    }
}

#[tokio::test]
async fn image_pre_execution_401_refreshes_once_on_the_same_account() {
    let (upstream, state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::UNAUTHORIZED,
            json!({"error":{"code":"token_expired"}}),
        ),
        Reply::Json(StatusCode::OK, json!({"data":[{"b64_json":"aW1hZ2U="}]})),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "relay-refresh-account",
            TokenSet::new(
                "synthetic-old-access",
                Some("refresh-secret".into()),
                None,
                Some(current_time_ms() + 600_000),
                current_time_ms(),
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let (gateway, events, refresh, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account(
            "relay-refresh-account",
            "synthetic-provider",
            &upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let response = reqwest::Client::new()
        .post(format!("{}/v1/images/generations", gateway.base_url))
        .bearer_auth(LOCAL_KEY)
        .json(&json!({"model":zenith_relay_core::IMAGE_API_MODEL,"prompt":"synthetic"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let _: Value = response.json().await.unwrap();
    assert_eq!(refresh.calls.load(Ordering::SeqCst), 1);
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].image_turn_id, requests[1].image_turn_id);
    assert!(
        requests
            .iter()
            .all(|request| request.path == "/v1/images/generations")
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].routing.as_ref().unwrap().attempt_outcome,
        Some(zenith_relay_core::AttemptOutcome::RetryAuthorization)
    );
    assert!(events[1].success);
}
