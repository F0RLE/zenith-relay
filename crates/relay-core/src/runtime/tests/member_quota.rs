use super::*;

#[test]
fn cooldowns_follow_upstream_scope_and_shared_member_resources() {
    use crate::scheduler::CooldownReason;
    let mut configured = RuntimeSource::unrestricted(source("multi", "synthetic", &["test"]));
    configured.protocol_config.capabilities = vec![
        ModelEndpointCapability {
            model_id: "test".into(),
            upstream_wire_api: WireApi::Responses,
            status: CapabilityStatus::Declared,
            origin: CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: BTreeMap::new(),
            reasoning_efforts: Vec::new(),
        },
        ModelEndpointCapability {
            model_id: "test".into(),
            upstream_wire_api: WireApi::Messages,
            status: CapabilityStatus::Declared,
            origin: CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: BTreeMap::new(),
            reasoning_efforts: Vec::new(),
        },
    ];
    let runtime = GatewayRuntime::from_pool(
        vec![configured],
        vec![RuntimeLocalKey::unrestricted(key("key", "synthetic-pool"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let route_upstreams = runtime
        .source_candidate_bindings
        .iter()
        .filter(|(_, binding)| binding.source_id == "multi")
        .map(|(id, binding)| {
            (
                id.clone(),
                binding.adapter.upstream_protocol(binding.wire_api),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(route_upstreams.len(), WireApi::ALL.len());
    let seed_upstream = route_upstreams
        .iter()
        .find(|(id, _)| id == "multi")
        .map(|(_, upstream)| *upstream)
        .unwrap();
    assert!(route_upstreams
        .iter()
        .any(|(id, upstream)| id != "multi" && *upstream == seed_upstream));
    assert!(route_upstreams
        .iter()
        .any(|(_, upstream)| *upstream != seed_upstream));
    for (reason, scope, shared) in [
        (CooldownReason::Mandatory, "test", false),
        (CooldownReason::RateLimit, "test", true),
        (CooldownReason::Mandatory, "*", true),
    ] {
        assert!(runtime.set_cooldown_with_reason_for_model_at(
            "multi",
            CooldownRequest {
                scope,

                retry_at_ms: 1000,
                reason,
            }
        ));
        let mut scheduler = runtime.lock_scheduler();
        for (id, upstream) in &route_upstreams {
            let cooled = scheduler
                .candidate(id)
                .unwrap()
                .cooldowns
                .contains_key(scope);
            assert_eq!(cooled, shared || *upstream == seed_upstream, "{id}");
            scheduler.clear_cooldown(id, scope);
        }
    }
}
#[tokio::test]
async fn pool_rotation_capacity_waits_wake_without_rebuilding_the_runtime() {
    let mut policy = crate::resolve_pool_routing(
        Some(&crate::PoolRoutingPolicy::default()),
        vec![
            (crate::PoolMemberKind::Source, "source-a".into(), 0, 1),
            (crate::PoolMemberKind::Source, "source-b".into(), 0, 1),
        ],
    );
    policy.mode = crate::PoolRoutingMode::InOrder;
    for member in &mut policy.members {
        member.max_concurrency = 1;
    }
    let runtime = GatewayRuntime::from_pool(
        ["source-a", "source-b"]
            .into_iter()
            .map(|id| RuntimeSource::unrestricted(source(id, "synthetic-upstream", &["gpt-test"])))
            .collect(),
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            pool_routing: Some(policy.clone()),
            max_retry_candidates: 2,
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    assert_eq!(runtime.request_dispatch_budget(), 2);
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let tried = HashSet::new();
    let select = || {
        runtime.select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &tried,
            (None, None),
            100,
        )
    };
    let (first, first_lease) = select().await.unwrap();
    let (second, second_lease) = select().await.unwrap();
    assert_eq!(first.candidate_id, "source-a");
    assert_eq!(second.candidate_id, "source-b");

    let mut waiting = Box::pin(select());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    drop(first_lease);
    let (released, released_lease) = tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(released.candidate_id, "source-a");

    let mut waiting = Box::pin(select());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    policy.members[1].max_concurrency = 2;
    assert!(runtime.set_pool_routing_policy(policy.clone(), 0).is_err());
    assert_eq!(runtime.request_dispatch_budget(), 2);
    assert!(futures_util::poll!(&mut waiting).is_pending());
    runtime.set_pool_routing_policy(policy, 6).unwrap();
    assert_eq!(runtime.request_dispatch_budget(), 6);
    let (expanded, expanded_lease) = tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(expanded.candidate_id, "source-b");

    let mut waiting = Box::pin(select());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    assert!(runtime.update_key_scope(
        "key-1",
        CandidateScope {
            source_ids: Some(Default::default()),
            account_ids: Some(Default::default()),
            ..Default::default()
        }
    ));
    assert!(tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        runtime
            .candidate_runtime_order()
            .iter()
            .map(|candidate| candidate.active_request_count)
            .sum::<u32>(),
        3
    );
    drop((second_lease, released_lease, expanded_lease));
    assert!(runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| candidate.active_request_count == 0));
}
#[tokio::test]
async fn recovery_probe_waits_for_its_owner_and_snapshots_match_activity_versions() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    let key = runtime.authenticated_key(&runtime.keys[0]);
    let tried = HashSet::new();
    let now = current_time_ms();
    let budget = crate::scheduler::rotation::SharedRequestBudget::for_incoming_request(3);
    let (_, failed) = runtime
        .select_and_reserve_with_budget(
            &key,
            "gpt-test",
            &[WireApi::Responses],
            &tried,
            (None, None),
            now - 1_000,
            &budget,
        )
        .await
        .unwrap();
    failed.begin_rotation_dispatch().unwrap();
    failed.settle_rotation_transient(None, now - 1_000);
    drop(failed);
    let events = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    runtime.set_activity_callback(move |event| captured.lock().unwrap().push(event));
    let select = || {
        runtime.select_and_reserve(
            &key,
            "gpt-test",
            &[WireApi::Responses],
            &tried,
            (None, None),
            now,
        )
    };
    let (_, probe) = select().await.unwrap();
    let mut waiting = Box::pin(select());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    let snapshot = runtime.candidate_runtime_order();
    let event = events.lock().unwrap()[0].clone();
    assert_eq!(snapshot[0].activity_revision, event.revision);
    assert_eq!(snapshot[0].runtime_id, event.runtime_id);
    assert_eq!(event.member_key, "account:account-1");
    assert!(event.runtime_id > 0);
    drop(probe);
    let (_, next_probe) = tokio::time::timeout(Duration::from_millis(500), waiting)
        .await
        .unwrap()
        .unwrap();
    drop(next_probe);
    let rebuilt = quota_runtime(QuotaSnapshot::default());
    assert!(rebuilt.candidate_runtime_order()[0].runtime_id > event.runtime_id);
    assert_eq!(rebuilt.candidate_runtime_order()[0].activity_revision, 0);
}
#[tokio::test]
async fn source_activity_reports_its_physical_member_on_admission_and_release() {
    let mut configured =
        RuntimeSource::unrestricted(source("source-1", "synthetic-upstream", &["gpt-test"]));
    configured.protocol_config.capabilities = vec![ModelEndpointCapability {
        model_id: "gpt-test".into(),
        upstream_wire_api: WireApi::Messages,
        status: CapabilityStatus::Declared,
        origin: CapabilityOrigin::Catalog,
        checked_at_ms: 1,
        features: BTreeMap::new(),
        reasoning_efforts: Vec::new(),
    }];
    let runtime = GatewayRuntime::from_pool(
        vec![configured],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let captured = Arc::new(Mutex::new(Vec::new()));
    let events = captured.clone();
    runtime.set_activity_callback(move |event| events.lock().unwrap().push(event));
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Messages],
            &HashSet::new(),
            (None, None),
            current_time_ms(),
        )
        .await
        .unwrap();
    assert!(selection.candidate_id.starts_with("source-1::"));
    assert_eq!(
        runtime.active_member_keys(current_time_ms(), 0),
        BTreeSet::from(["source:source-1".into()])
    );
    lease.begin_rotation_dispatch().unwrap();
    drop(lease);
    assert!(runtime.active_member_keys(current_time_ms(), 0).is_empty());
    assert_eq!(
        runtime.active_member_keys(current_time_ms(), 600_000),
        BTreeSet::from(["source:source-1".into()])
    );
    assert!(runtime
        .active_member_keys(current_time_ms() + 600_000, 600_000)
        .is_empty());
    let events = captured.lock().unwrap();
    assert_eq!(events.len(), 2);
    assert!(events
        .iter()
        .all(|event| event.candidate_id == selection.candidate_id
            && event.member_key == "source:source-1"));
    assert!(events[0].in_flight > 0);
    assert_eq!(events[1].in_flight, 0);
}
#[test]
fn chatgpt_team_breaker_blocks_siblings_and_deduplicates() {
    let first = quota_account(QuotaSnapshot::default());
    let mut sibling = first.clone();
    sibling.id = "account-2".to_string();
    sibling.chatgpt_account_id = "team-1".to_string();
    let mut first = first;
    first.chatgpt_account_id = "team-1".to_string();
    let runtime = GatewayRuntime::from_mixed_pool(
        Vec::new(),
        vec![first, sibling],
        vec![RuntimeMixedLocalKey {
            key: key("key-1", "local-secret"),
            enabled: true,
            source_ids: None,
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            wire_apis: None,
        }],
        RuntimeChatGptAuth {
            token_authority: Arc::new(TokenAuthority::new(1).unwrap()),
            refresh_adapter: Arc::new(NeverRefresh),
            persistence_adapter: Arc::new(NoopPersistence),
            refresh_skew_ms: 60_000,
            agent_identities: HashMap::new(),
        },
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    let persisted = Arc::new(Mutex::new(Vec::<Vec<String>>::new()));
    let capture = persisted.clone();
    runtime.set_chatgpt_team_breaker_callback(move |ids| {
        capture.lock().unwrap().push(ids);
    });

    assert!(runtime.trip_chatgpt_team_breaker("account-1", 1_000));
    let snapshots = runtime
        .candidate_runtime_order()
        .into_iter()
        .map(|snapshot| (snapshot.candidate_id, snapshot.available))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(snapshots.get("account-1"), Some(&true));
    assert_eq!(snapshots.get("account-2"), Some(&false));
    assert_eq!(
        persisted.lock().unwrap().as_slice(),
        &[vec!["account-2".to_string()]]
    );
    assert!(!runtime.trip_chatgpt_team_breaker("account-1", 2_000));
}
#[test]
fn passive_quota_exhaustion_and_recovery_are_persisted_without_waiting_for_the_debounce() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    let mut exhausted_headers = reqwest::header::HeaderMap::new();
    exhausted_headers.insert(
        "x-codex-primary-used-percent",
        reqwest::header::HeaderValue::from_static("100"),
    );
    exhausted_headers.insert(
        "x-codex-primary-reset-after-seconds",
        reqwest::header::HeaderValue::from_static("1"),
    );

    assert!(runtime.observe_codex_quota_headers(
        "account-1",
        reqwest::StatusCode::OK,
        &exhausted_headers,
        1_000,
    ));
    let exhausted = runtime
        .take_passive_quota_snapshot("account-1", 1_000)
        .unwrap();
    assert!(exhausted.limit_reached);
    assert_eq!(
        CandidateQuota::from_snapshot(&exhausted, 1_000, QUOTA_STALE_AFTER_MS),
        CandidateQuota::Exhausted
    );

    let mut recovered_headers = reqwest::header::HeaderMap::new();
    recovered_headers.insert(
        "x-codex-primary-used-percent",
        reqwest::header::HeaderValue::from_static("5"),
    );
    recovered_headers.insert(
        "x-codex-primary-reset-after-seconds",
        reqwest::header::HeaderValue::from_static("600"),
    );

    assert!(runtime.observe_codex_quota_headers(
        "account-1",
        reqwest::StatusCode::OK,
        &recovered_headers,
        3_000,
    ));
    let persisted = runtime
        .take_passive_quota_snapshot("account-1", 3_000)
        .unwrap();

    assert!(!persisted.limit_reached);
    assert_ne!(
        CandidateQuota::from_snapshot(&persisted, 3_000, QUOTA_STALE_AFTER_MS),
        CandidateQuota::Exhausted
    );
    assert!(runtime
        .take_passive_quota_snapshot("account-1", 3_001)
        .is_none());
}
#[test]
fn refreshed_quota_snapshot_replaces_passive_state_before_header_merges() {
    let initial = QuotaSnapshot {
        available_credits_micro_units: Some(100),
        provider_credits_available: true,
        updated_at_ms: Some(1_000),
        ..QuotaSnapshot::default()
    };
    let runtime = quota_runtime(initial);
    let refreshed = QuotaSnapshot {
        available_credits_micro_units: Some(200),
        provider_credits_available: true,
        updated_at_ms: Some(2_000),
        ..QuotaSnapshot::default()
    };

    assert!(runtime.sync_account_quota_snapshot("account-1", &refreshed, 2_000));

    // Response headers do not carry the provider credit ledger. They must be
    // merged into the refreshed snapshot rather than the pre-refresh one.
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        "x-codex-primary-used-percent",
        reqwest::header::HeaderValue::from_static("10"),
    );
    assert!(runtime.observe_codex_quota_headers(
        "account-1",
        reqwest::StatusCode::OK,
        &headers,
        3_000,
    ));

    let merged = runtime
        .take_passive_quota_snapshot("account-1", 8_000)
        .expect("header merge should become persistable");
    assert_eq!(merged.available_credits_micro_units, Some(200));
    assert!(merged.provider_credits_available);
}
#[test]
fn quota_429_does_not_turn_a_slot_into_permanent_exhaustion() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    runtime.apply_usage_event(
        &UsageEvent {
            request_id: "request".into(),
            attempt: 1,
            local_key_id: "key-1".into(),
            source_id: "openai-codex".into(),
            candidate_id: Some("account-1".into()),
            account_id: Some("account-1".into()),
            account_token_generation: None,
            client_context_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: false,
            http_status: reqwest::StatusCode::TOO_MANY_REQUESTS.as_u16(),
            error_category: Some("upstream_quota_exhausted".into()),
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: Some("*".into()),
            retry_at_ms: Some(2_000),
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
        },
        1_000,
    );

    let snapshot = runtime
        .candidate_runtime_order()
        .into_iter()
        .find(|candidate| candidate.candidate_id == "account-1")
        .unwrap();
    assert!(snapshot.available);
}

#[test]
fn reported_quota_exhaustion_zeroes_an_open_primary_window_without_inventing_a_limit() {
    use crate::quota::{QuotaWindow, QuotaWindowKind};

    let observed_at_ms = crate::unix_time_ms();
    let window = |kind, available| QuotaWindow {
        kind,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(available),
        explicitly_full: None,
        reset_at_ms: Some(observed_at_ms.saturating_add(60_000)),
        window_minutes: Some(300),
        observed_at_ms,
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: None,
    };
    let runtime = quota_runtime(QuotaSnapshot {
        primary: Some(window(QuotaWindowKind::Primary, 900)),
        secondary: Some(window(QuotaWindowKind::Secondary, 2_900)),
        updated_at_ms: Some(observed_at_ms),
        ..QuotaSnapshot::default()
    });
    runtime.apply_usage_event(
        &UsageEvent {
            request_id: "request".into(),
            attempt: 1,
            local_key_id: "key-1".into(),
            source_id: "openai-codex".into(),
            candidate_id: Some("account-1".into()),
            account_id: Some("account-1".into()),
            account_token_generation: None,
            client_context_id: None,
            routing: None,
            requested_model: Some("gpt-test".into()),
            resolved_model: Some("gpt-test".into()),
            requested_reasoning_effort: None,
            effective_reasoning_effort: None,
            wire_api: WireApi::Responses,
            service_tier: DefaultServiceTier::Standard,
            applied_service_tier: None,
            success: false,
            http_status: reqwest::StatusCode::FORBIDDEN.as_u16(),
            error_category: Some("upstream_quota_exhausted".into()),
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: Some("*".into()),
            retry_at_ms: Some(observed_at_ms.saturating_add(1_000)),
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
        },
        observed_at_ms,
    );

    let persisted = runtime
        .take_passive_quota_snapshot("account-1", observed_at_ms)
        .expect("reported primary exhaustion should persist immediately");
    assert_eq!(
        persisted.primary.as_ref().unwrap().available_basis_points,
        Some(0)
    );
    assert_eq!(
        persisted.secondary.as_ref().unwrap().available_basis_points,
        Some(2_900)
    );
    assert!(!persisted.limit_reached);
    assert_eq!(
        CandidateQuota::from_snapshot(&persisted, observed_at_ms, QUOTA_STALE_AFTER_MS),
        CandidateQuota::Exhausted
    );
    let snapshot = runtime
        .candidate_runtime_order()
        .into_iter()
        .find(|candidate| candidate.candidate_id == "account-1")
        .unwrap();
    assert!(!snapshot.available);
}
