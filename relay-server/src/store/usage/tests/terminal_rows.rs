use super::*;

#[test]
fn usage_keeps_one_terminal_row_per_request() {
    let root = test_root("usage-terminal-row");
    let path = root.join("relay.sqlite");
    let store = Store::open(path.clone()).unwrap();
    let mut event = UsageEvent {
        request_id: "req_fallback".into(),
        attempt: 1,
        local_key_id: "key".into(),
        source_id: "source_1".into(),
        candidate_id: Some("source_1".into()),
        account_id: None,
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-test".into()),
        resolved_model: Some("gpt-test".into()),
        requested_reasoning_effort: Some("max".into()),
        effective_reasoning_effort: Some("max".into()),
        wire_api: WireApi::Responses,
        transport: zenith_relay_core::UsageTransport::Http,
        service_tier: DefaultServiceTier::Fast,
        applied_service_tier: None,
        success: false,
        http_status: 503,
        error_category: Some("upstream_unavailable".into()),
        tool_use: ToolUseDiagnostics {
            client_tool_count: 73,
            forwarded_tool_count: 73,
            tool_choice: ToolChoiceMode::Auto,
            tool_call_count: 1,
            text_output: false,
            terminal_output: TerminalOutputKind::ToolCall,
            client_schema_bytes: Some(12345),
            forwarded_schema_bytes: Some(12345),
            filtered_tool_count: 0,
            policy_mode: Some(zenith_relay_core::ToolPolicyMode::Automatic),
            policy_outcome: Some(zenith_relay_core::ToolPolicyOutcome::Deferred),
            policy_fallback: false,
            deferred_tool_search: true,
        },
        cooldown_scope: Some("*".into()),
        retry_at_ms: Some(60_000),
        consecutive_failures: Some(1),
        latency_ms: 1,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: None,
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: None,
        total_tokens: None,
        upstream_error: None,
        quota_snapshot: None,
    };
    event.upstream_error = Some(
        zenith_relay_core::usage::UpstreamErrorDetails::from_response_body(
            Some(503),
            br#"{"error":{"code":"future_capacity","message":"Capacity temporarily exhausted"}}"#,
        ),
    );
    event.upstream_error.as_mut().unwrap().message =
        Some("Capacity exhausted; Bearer synthetic-private".into());
    store.record_usage(&event, 1_000).unwrap();
    let stored: String = store
        .lock()
        .unwrap()
        .query_row("SELECT upstream_error_json FROM usage_events", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert!(!stored.contains("synthetic-private"));
    event.attempt = 2;
    event.source_id = "source_2".into();
    event.candidate_id = Some("source_2".into());
    event.success = true;
    event.http_status = 200;
    event.error_category = None;
    event.effective_reasoning_effort = Some("low".into());
    event.cooldown_scope = None;
    event.retry_at_ms = None;
    event.consecutive_failures = Some(0);
    event.applied_service_tier = Some("flex".into());
    event.total_tokens = Some(10);
    store.record_usage(&event, 2_000).unwrap();

    event.request_id = "req_failed".into();
    event.attempt = 1;
    event.success = false;
    event.http_status = 503;
    event.error_category = Some("upstream_unavailable".into());
    event.requested_reasoning_effort = Some("not-a-reasoning-effort".into());
    event.effective_reasoning_effort = None;
    event.total_tokens = None;
    store.record_usage(&event, 3_000).unwrap();
    event.attempt = 2;
    event.http_status = 429;
    event.error_category = Some("upstream_rate_limited".into());
    store.record_usage(&event, 4_000).unwrap();

    drop(store);
    let store = Store::open(path).unwrap();
    let page = store.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(page.total, 2);
    let fallback = page
        .events
        .iter()
        .find(|event| event.request_id == "req_fallback")
        .unwrap();
    assert!(fallback.success);
    assert_eq!(fallback.tool_use.as_ref(), Some(&event.tool_use));
    assert!(fallback.upstream_error.is_none());
    assert_eq!(fallback.http_status, 200);
    assert_eq!(fallback.service_tier, DefaultServiceTier::Fast);
    assert_eq!(fallback.applied_service_tier, Some("flex".into()));
    assert_eq!(fallback.requested_reasoning_effort.as_deref(), Some("max"));
    assert_eq!(fallback.effective_reasoning_effort.as_deref(), Some("low"));
    assert_eq!(
        fallback
            .tool_use
            .as_ref()
            .map(|tool_use| tool_use.tool_call_count),
        Some(1)
    );
    let failed = page
        .events
        .iter()
        .find(|event| event.request_id == "req_failed")
        .unwrap();
    assert!(!failed.success);
    assert_eq!(
        failed.upstream_error,
        event
            .upstream_error
            .as_ref()
            .map(|details| details.sanitized())
    );
    assert_eq!(failed.http_status, 429);
    assert_eq!(failed.error_origin, Some(ErrorOrigin::Provider));
    assert_eq!(failed.requested_reasoning_effort, None);
    assert_eq!(failed.effective_reasoning_effort, None);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn deleting_account_removes_telemetry_and_rejects_late_usage() {
    let root = test_root("account-delete");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    let account_id = "account-delete";
    store
        .save_account(&ServerAccountRecord {
            id: account_id.into(),
            label: "Delete me".into(),
            identity_hint: "deleted".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            source_id: "codex".into(),
            secret_ref: "account:delete".into(),
            provider_family: Some("openai".into()),
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            models: vec!["gpt-test".into()],
            discovered_models: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
            purchase_cost_micro_usd: None,
            cooldowns: BTreeMap::new(),
            consecutive_failures: 0,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
            proxy_id: None,
            bypass_common_proxy: false,
        })
        .unwrap();
    let mut event = UsageEvent {
        request_id: "request-before-delete".into(),
        attempt: 1,
        local_key_id: "key".into(),
        source_id: "codex".into(),
        candidate_id: Some(account_id.into()),
        account_id: Some(account_id.into()),
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-5.4".into()),
        resolved_model: Some("gpt-5.4".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: zenith_relay_core::UsageTransport::Http,
        service_tier: DefaultServiceTier::Fast,
        applied_service_tier: Some("default".into()),
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: Some(0),
        latency_ms: 1,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: Some(2),
        cached_input_tokens: Some(1),
        cache_write_input_tokens: Some(1),
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: Some(3),
        total_tokens: Some(5),
        upstream_error: None,
        quota_snapshot: None,
    };
    store.record_usage(&event, 10).unwrap();
    let usage = store.usage_page(&UsageQuery::default()).unwrap();
    // The account event carries a measured value, and the totals are that
    // same value merged once. Asserting the relation instead of a catalog
    // price keeps the test stable when prices move.
    assert!(usage.events[0].api_equivalent.micro_usd > 0);
    assert_eq!(usage.totals.api_equivalent, usage.events[0].api_equivalent);
    assert!(usage.events[0].tool_use.is_none());
    let candidate_hint = hex::encode(Sha256::digest(account_id.as_bytes()))[..12].to_string();
    store
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO usage_candidate_rollups(candidate_kind, candidate_id, model)
             VALUES ('account', ?1, 'gpt-test')",
            [&candidate_hint],
        )
        .unwrap();
    store
        .upsert(&ResponseAffinityBinding {
            key: "response-delete".into(),
            candidate_id: account_id.into(),
            expires_at_ms: 1_000,
        })
        .unwrap();

    assert!(store.delete_account(account_id).unwrap().is_some());
    assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 0);
    assert!(!store
        .api_equivalents()
        .unwrap()
        .contains_key(&candidate_hint));
    assert!(store.find("response-delete", 1).unwrap().is_none());

    event.request_id = "request-after-delete".into();
    store.record_usage(&event, 20).unwrap();
    assert_eq!(store.usage_page(&UsageQuery::default()).unwrap().total, 0);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn quota_window_equivalent_uses_only_that_account_and_window() {
    let root = test_root("quota-window-equivalent");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    for account_id in ["account-a", "account-b"] {
        store
            .save_account(&ServerAccountRecord {
                id: account_id.into(),
                label: account_id.into(),
                identity_hint: account_id.into(),
                enabled: true,
                in_pool: true,
                draining: false,
                source_id: "codex".into(),
                secret_ref: format!("account:{account_id}"),
                provider_family: Some("openai".into()),
                auth_state: AccountAuthState::Active,
                health: AccountHealthState::Healthy,
                models: vec!["gpt-5.4".into()],
                discovered_models: None,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                subscription: Subscription::default(),
                quota: QuotaSnapshot::default(),
                purchase_cost_micro_usd: None,
                cooldowns: BTreeMap::new(),
                consecutive_failures: 0,
                created_at_ms: 1,
                last_used_at_ms: None,
                last_error_code: None,
                proxy_id: None,
                bypass_common_proxy: false,
            })
            .unwrap();
    }
    let mut event = UsageEvent {
        request_id: "inside".into(),
        attempt: 1,
        local_key_id: "key".into(),
        source_id: "source".into(),
        candidate_id: Some("account-a".into()),
        account_id: Some("account-a".into()),
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-5.4".into()),
        resolved_model: Some("gpt-5.4".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: zenith_relay_core::UsageTransport::Http,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: Some(0),
        latency_ms: 10,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: Some(1_000),
        cached_input_tokens: Some(0),
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: Some(100),
        total_tokens: Some(1_100),
        upstream_error: None,
        quota_snapshot: None,
    };
    store.record_usage(&event, 5_000).unwrap();
    event.request_id = "before-window".into();
    event.input_tokens = Some(9_000_000);
    event.output_tokens = Some(9_000_000);
    event.total_tokens = Some(18_000_000);
    store.record_usage(&event, 1_000).unwrap();
    event.request_id = "other-account".into();
    event.account_id = Some("account-b".into());
    event.candidate_id = Some("account-b".into());
    event.input_tokens = Some(8_000_000);
    event.output_tokens = Some(8_000_000);
    event.total_tokens = Some(16_000_000);
    store.record_usage(&event, 5_000).unwrap();

    let hint = identity_hint("account-a");
    let priced = store
        .quota_window_equivalents_with_pricing(
            &[(hint.clone(), 4_000, 6_000)],
            &test_pricing_catalog(),
            &test_pricing_context(
                &store.model_price_overrides().unwrap(),
                &store.source_price_overrides().unwrap(),
            ),
        )
        .unwrap();
    let page = store
        .usage_page(&UsageQuery {
            from_ms: Some(4_000),
            to_ms: Some(6_000),
            source_or_account_query: Some(hint.clone()),
            include_events: Some(false),
            include_models: Some(false),
            include_pool_members: Some(false),
            ..UsageQuery::default()
        })
        .unwrap();
    assert_eq!(priced[&hint], page.totals.api_equivalent);
    assert!(priced[&hint].micro_usd > 0);
    assert!(store.api_equivalents().unwrap()[&hint].micro_usd > priced[&hint].micro_usd);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
