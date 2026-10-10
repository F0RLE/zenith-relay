use super::*;

#[tokio::test]
async fn account_non_stream_client_buffers_codex_stream_response() {
    let (upstream, state) = spawn_upstream(vec![Reply::Stream(vec![
        StreamChunk::Data(
            "data: {\"type\":\"response.in_progress\",\"response\":{\"id\":\"early-response\",\"object\":\"response\",\"status\":\"in_progress\",\"output\":[]}}\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"id\":\"message\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[]}}\n\n",
        ),
        StreamChunk::Data(
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"account-response\",\"object\":\"response\",\"model\":\"gpt-p3\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n",
        ),
        StreamChunk::Data("data: [DONE]\n\n"),
    ])])
    .await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("relay-account", "provider-account", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_TYPE).unwrap(),
        "application/json"
    );
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["id"], "account-response");
    assert_eq!(body["output"][0]["type"], "message");
    let requests = state.requests.lock().unwrap();
    assert_eq!(requests[0].body["store"], false);
    assert_eq!(requests[0].body["stream"], true);
    assert!(requests[0].body["input"].is_array());
    assert!(requests[0].body.get("max_output_tokens").is_none());
    drop(requests);
    let events = events.lock().unwrap();
    assert!(events[0].success);
    assert_eq!(events[0].total_tokens, Some(2));
}

#[tokio::test]
async fn high_priority_api_source_runs_before_a_healthy_oauth_account() {
    let (source_upstream, source_state) = spawn_upstream(vec![success_reply("api-first")]).await;
    let (account_upstream, account_state) =
        spawn_upstream(vec![success_reply("oauth-must-not-run")]).await;
    let authority = ready_authority("oauth-account", "oauth-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source(
            "api-source",
            &source_upstream,
            "source-key",
            1_000_000,
        )],
        vec![account(
            "oauth-account",
            "provider-account",
            &account_upstream,
            0,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.json::<Value>().await.unwrap()["id"], "api-first");
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    assert!(account_state.requests.lock().unwrap().is_empty());
}

#[tokio::test]
async fn automatic_mode_does_not_treat_negative_api_priority_as_a_type_gate() {
    let (source_upstream, source_state) = spawn_upstream(vec![success_reply("api-ready")]).await;
    let (account_upstream, account_state) =
        spawn_upstream(vec![success_reply("oauth-ready")]).await;
    let authority = ready_authority("oauth-account", "oauth-access").await;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        vec![source(
            "api-source",
            &source_upstream,
            "source-key",
            -1_000_000,
        )],
        vec![account(
            "oauth-account",
            "provider-account",
            &account_upstream,
            0,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let mut ids = BTreeSet::new();
    for _ in 0..2 {
        let response = request(&gateway, false).await;
        assert_eq!(response.status(), StatusCode::OK);
        ids.insert(
            response.json::<Value>().await.unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    assert_eq!(
        ids,
        BTreeSet::from(["api-ready".into(), "oauth-ready".into()])
    );
    assert_eq!(account_state.requests.lock().unwrap().len(), 1);
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn rejected_account_falls_back_to_api_source_in_the_same_scheduler() {
    let (account_upstream, account_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"code": "server_is_overloaded"}}),
    )])
    .await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("source-response")]).await;
    let authority = ready_authority("relay-account", "account-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("source", &source_upstream, "source-key", 0)],
        vec![account(
            "relay-account",
            "provider-account",
            &account_upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "source-response"
    );
    assert_eq!(account_state.requests.lock().unwrap().len(), 1);
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    assert_eq!(
        source_state.requests.lock().unwrap()[0]
            .authorization
            .as_deref(),
        Some("Bearer source-key")
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].account_id.as_deref(), Some("relay-account"));
    assert_eq!(events[1].account_id, None);
    assert_eq!(events[1].candidate_id.as_deref(), Some("source"));
}

#[tokio::test]
async fn quota_limited_account_falls_back_to_api_source_for_a_new_request() {
    let (account_upstream, account_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::TOO_MANY_REQUESTS,
        json!({"error": {"type": "usage_limit_reached", "message": "Usage limit reached"}}),
    )])
    .await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("source-after-quota")]).await;
    let authority = ready_authority("quota-account", "quota-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("source", &source_upstream, "source-key", 0)],
        vec![account(
            "quota-account",
            "provider-quota",
            &account_upstream,
            10,
        )],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "source-after-quota"
    );
    assert_eq!(account_state.requests.lock().unwrap().len(), 1);
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[0].error_category.as_deref(),
        Some("upstream_quota_exhausted")
    );
    assert!(events[1].success);
}

#[tokio::test]
async fn terminal_account_auth_is_removed_from_following_requests() {
    let (broken_upstream, broken_state) = spawn_upstream(Vec::new()).await;
    let (ready_upstream, ready_state) = spawn_upstream(vec![
        success_reply("ready-first"),
        success_reply("ready-second"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    authority
        .register(
            "oauth-broken",
            TokenSet::access_only("expired-access", Some(1), 0).unwrap(),
            AccountAuthState::RequiresReauth(ReauthReason::InvalidatedRefreshToken),
        )
        .await
        .unwrap();
    register_ready(&authority, "oauth-ready", "ready-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account("oauth-broken", "provider-broken", &broken_upstream, 9_000),
            account("oauth-ready", "provider-ready", &ready_upstream, 8_000),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert_eq!(request(&gateway, false).await.status(), StatusCode::OK);
    assert!(broken_state.requests.lock().unwrap().is_empty());
    assert_eq!(ready_state.requests.lock().unwrap().len(), 2);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(
        events
            .iter()
            .filter(|event| event.account_id.as_deref() == Some("oauth-broken"))
            .count(),
        1
    );
    assert_eq!(events[0].error_category.as_deref(), Some("account_auth"));
    assert!(
        !gateway
            .runtime
            .as_ref()
            .unwrap()
            .candidate_runtime_order()
            .iter()
            .find(|candidate| candidate.candidate_id == "oauth-broken")
            .unwrap()
            .available
    );
}

#[tokio::test]
async fn rejected_account_follows_explicit_member_order_before_api_source() {
    let (primary_upstream, primary_state) = spawn_upstream(vec![Reply::Json(
        StatusCode::SERVICE_UNAVAILABLE,
        json!({"error": {"code": "server_is_overloaded"}}),
    )])
    .await;
    let (secondary_upstream, secondary_state) =
        spawn_upstream(vec![success_reply("secondary-account")]).await;
    let (paid_upstream, paid_state) =
        spawn_upstream(vec![success_reply("paid-source-must-not-run")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "oauth-primary", "primary-access").await;
    register_ready(&authority, "oauth-secondary", "secondary-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source("paid-source", &paid_upstream, "paid-key", 100)],
        vec![
            account("oauth-primary", "provider-primary", &primary_upstream, 300),
            account(
                "oauth-secondary",
                "provider-secondary",
                &secondary_upstream,
                200,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(
        &gateway,
        &["oauth-primary", "oauth-secondary", "paid-source"],
    );

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "secondary-account"
    );
    assert_eq!(primary_state.requests.lock().unwrap().len(), 1);
    assert_eq!(secondary_state.requests.lock().unwrap().len(), 1);
    assert!(paid_state.requests.lock().unwrap().is_empty());
    assert_eq!(
        secondary_state.requests.lock().unwrap()[0]
            .authorization
            .as_deref(),
        Some("Bearer secondary-access")
    );
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id.as_deref(), Some("oauth-primary"));
    assert_eq!(events[1].candidate_id.as_deref(), Some("oauth-secondary"));
}

#[tokio::test]
async fn unavailable_accounts_do_not_block_healthy_account_or_source_fallback() {
    let (exhausted_upstream, exhausted_state) =
        spawn_upstream(vec![success_reply("exhausted-must-not-run")]).await;
    let (reauth_upstream, reauth_state) =
        spawn_upstream(vec![success_reply("reauth-must-not-run")]).await;
    let (eligible_upstream, eligible_state) = spawn_upstream(vec![
        success_reply("healthy-account"),
        Reply::Json(
            StatusCode::SERVICE_UNAVAILABLE,
            json!({"error": {"code": "server_is_overloaded"}}),
        ),
    ])
    .await;
    let (source_upstream, source_state) =
        spawn_upstream(vec![success_reply("source-fallback")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "oauth-exhausted", "exhausted-access").await;
    register_ready(&authority, "oauth-reauth", "reauth-access").await;
    register_ready(&authority, "oauth-eligible", "eligible-access").await;
    let mut exhausted = account(
        "oauth-exhausted",
        "provider-exhausted",
        &exhausted_upstream,
        400,
    );
    exhausted.quota = CandidateQuota::Exhausted;
    let mut reauth = account("oauth-reauth", "provider-reauth", &reauth_upstream, 300);
    reauth.health = CandidateHealth::ReauthRequired;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        vec![source(
            "source-fallback",
            &source_upstream,
            "source-key",
            100,
        )],
        vec![
            exhausted,
            reauth,
            account(
                "oauth-eligible",
                "provider-eligible",
                &eligible_upstream,
                200,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(
        &gateway,
        &[
            "oauth-eligible",
            "source-fallback",
            "oauth-exhausted",
            "oauth-reauth",
        ],
    );

    let healthy_response = request(&gateway, false).await;
    assert_eq!(healthy_response.status(), StatusCode::OK);
    assert_eq!(
        healthy_response.json::<Value>().await.unwrap()["id"],
        "healthy-account"
    );
    assert!(source_state.requests.lock().unwrap().is_empty());

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "source-fallback"
    );
    assert!(exhausted_state.requests.lock().unwrap().is_empty());
    assert!(reauth_state.requests.lock().unwrap().is_empty());
    // Manual rotation advances to the next eligible candidate between
    // independent requests, so the source fallback is selected directly on
    // the second request instead of probing the account again first.
    assert_eq!(eligible_state.requests.lock().unwrap().len(), 1);
    assert_eq!(source_state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].candidate_id.as_deref(), Some("oauth-eligible"));
    assert!(events[0].success);
    assert_eq!(events[1].candidate_id.as_deref(), Some("source-fallback"));
    assert!(events[1].success);
}

#[tokio::test]
async fn exhausted_account_keeps_codex_catalog_but_does_not_receive_requests() {
    let (upstream, state) = spawn_upstream(vec![success_reply("must-not-run")]).await;
    let authority = ready_authority("oauth-exhausted", "exhausted-access").await;
    let mut exhausted = account("oauth-exhausted", "provider-exhausted", &upstream, 100);
    exhausted.quota = CandidateQuota::Exhausted;
    let (gateway, _, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![exhausted],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let catalog: Value = reqwest::Client::new()
        .get(format!(
            "{}/v1/models?client_version={CODEX_MODELS_CLIENT_VERSION}",
            gateway.base_url,
        ))
        .bearer_auth(LOCAL_KEY)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(catalog["models"][0]["slug"], MODEL);
    assert!(catalog["models"][0].get("service_tiers").is_some());

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response.json::<Value>().await.unwrap()["error"]["code"],
        "no_eligible_source"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn persisted_account_cooldown_and_failure_count_are_ignored() {
    let (upstream, state) = spawn_upstream(vec![success_reply("account-ready")]).await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "oauth-account", "account-access").await;
    let mut account = account("oauth-account", "provider-account", &upstream, 300);
    account
        .cooldowns
        .insert(MODEL.to_string(), current_time_ms().saturating_add(60_000));
    account.consecutive_failures = 7;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;

    let response = request(&gateway, false).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "account-ready"
    );
    assert_eq!(state.requests.lock().unwrap().len(), 1);
    let events = events.lock().unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].success);
    assert_eq!(events[0].consecutive_failures, Some(0));
}

#[tokio::test]
async fn http_usage_limit_immediately_excludes_the_account_until_quota_refresh() {
    let reset_at = current_time_ms() / 1_000 + 60 * 60;
    let (limited_upstream, limited_state) = spawn_upstream(vec![
        Reply::Json(
            StatusCode::TOO_MANY_REQUESTS,
            json!({"error": {
                "type": "usage_limit_reached",
                "message": "Usage limit reached",
                "resets_at": reset_at
            }}),
        ),
        success_reply("recovered"),
    ])
    .await;
    let (fallback_upstream, fallback_state) = spawn_upstream(vec![
        success_reply("fallback-1"),
        success_reply("fallback-2"),
    ])
    .await;
    let authority = Arc::new(TokenAuthority::new(4).unwrap());
    register_ready(&authority, "limited-account", "limited-access").await;
    register_ready(&authority, "fallback-account", "fallback-access").await;
    let (gateway, events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![
            account(
                "limited-account",
                "provider-limited",
                &limited_upstream,
                200,
            ),
            account(
                "fallback-account",
                "provider-fallback",
                &fallback_upstream,
                100,
            ),
        ],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    rotation_policy::set_order(&gateway, &["limited-account", "fallback-account"]);

    let first = request(&gateway, false).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.json::<Value>().await.unwrap()["id"], "fallback-1");
    let second = request(&gateway, false).await;
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(second.json::<Value>().await.unwrap()["id"], "fallback-2");
    assert_eq!(limited_state.requests.lock().unwrap().len(), 1);
    assert_eq!(fallback_state.requests.lock().unwrap().len(), 2);

    let events = events.lock().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].candidate_id.as_deref(), Some("limited-account"));
    assert_eq!(events[0].cooldown_scope.as_deref(), Some("*"));
    assert!(events[0].retry_at_ms.is_some());
    assert_eq!(events[0].consecutive_failures, Some(0));
    assert_eq!(events[2].candidate_id.as_deref(), Some("fallback-account"));
    assert!(events[2].success);
    assert_eq!(events[2].cooldown_scope, None);
    assert_eq!(events[2].retry_at_ms, None);
}

#[tokio::test]
async fn managed_http_retry_becomes_persistent_when_enabled_mid_request() {
    let (upstream, upstream_state) = spawn_delayed_upstream_with_replies(
        vec![
            Reply::JsonWithHeaders(
                StatusCode::TOO_MANY_REQUESTS,
                json!({"error": {"code": "rate_limit_exceeded"}}),
                vec![("retry-after", "2")],
            ),
            success_reply("persistent-recovered"),
        ],
        Duration::from_millis(100),
    )
    .await;
    let authority = ready_authority("toggle-account", "toggle-access").await;
    let (gateway, _events, _, _) = spawn_mixed_gateway(
        Vec::new(),
        vec![account("toggle-account", "provider-toggle", &upstream, 10)],
        vec![mixed_key(None, None)],
        authority,
        refresh_adapter(),
        Arc::new(PersistenceAdapter::default()),
    )
    .await;
    let runtime = gateway.runtime.as_ref().unwrap().clone();
    let gateway_url = gateway.base_url.clone();
    let pending_response = tokio::spawn(async move {
        reqwest::Client::new()
            .post(format!("{gateway_url}/v1/responses"))
            .bearer_auth(LOCAL_KEY)
            .header("originator", "codex_cli_rs")
            .json(&json!({"model": MODEL, "input": "wait for a route"}))
            .send()
            .await
            .unwrap()
    });

    tokio::time::timeout(Duration::from_secs(2), async {
        while upstream_state.requests.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("managed request did not reach the upstream");
    // The delayed first response gives the in-flight request time to observe
    // this live setting before its retry decision.
    runtime.set_route_recovery_enabled(true);

    let response = tokio::time::timeout(Duration::from_secs(5), pending_response)
        .await
        .expect("persistent retry used its old bounded deadline")
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.json::<Value>().await.unwrap()["id"],
        "persistent-recovered"
    );
    assert_eq!(upstream_state.requests.lock().unwrap().len(), 2);
}
