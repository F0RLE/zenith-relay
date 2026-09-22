use super::*;
use crate::accounts::{
    AccountAuthState, TokenPersistenceAdapter, TokenPersistenceFailure, TokenRefresh,
    TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
};
use crate::{
    CandidateHealth, CandidateQuota, CapabilityOrigin, CapabilityStatus, ModelEndpointCapability,
    ToolUseDiagnostics, QUOTA_STALE_AFTER_MS,
};
use futures_util::future::BoxFuture;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

#[test]
fn provider_image_metadata_does_not_change_unknown_model_capabilities() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    runtime.remember_codex_model_manifest("account-1", serde_json::json!({"models":[
        {"slug":"gpt-test", "input_modalities":["text"], "supported_reasoning_levels":[{"effort":"high","description":"high"}]}
    ]}), current_time_ms());
    let capabilities = runtime.model_capabilities("gpt-test");
    assert_eq!(capabilities.input_modalities, ["text", "image"]);
    assert!(capabilities.reasoning_effort_levels.is_empty());
    assert_eq!(capabilities.tool_call, Some(true));
}

#[test]
fn client_reasoning_projection_respects_source_scope_and_adapter_limits() {
    let sources = [
        ("messages", WireApi::Messages),
        ("native", WireApi::Responses),
    ]
    .map(|(id, upstream)| {
        let mut configured = RuntimeSource::unrestricted(source(id, "synthetic", &["test"]));
        configured.protocol_config.capabilities = vec![ModelEndpointCapability {
            model_id: "test".into(),
            upstream_wire_api: upstream,
            status: CapabilityStatus::Declared,
            origin: CapabilityOrigin::Catalog,
            checked_at_ms: 1,
            features: BTreeMap::new(),
            reasoning_efforts: vec!["low".into(), "xhigh".into()],
        }];
        configured
    });
    let runtime = GatewayRuntime::from_pool(
        sources.into(),
        vec![RuntimeLocalKey {
            source_ids: Some(vec!["messages".into()]),
            ..RuntimeLocalKey::unrestricted(key("key", "synthetic-pool"))
        }],
        GatewayRuntimeOptions {
            model_metadata_catalog: Some(crate::model_metadata::ModelMetadataCatalogHandle::new(
                crate::model_metadata::ModelMetadataCatalog::from_models_dev_json(
                    r#"{"test/test":{"reasoning":true,"reasoning_effort_levels":["low","xhigh"]}}"#,
                )
                .unwrap(),
            )),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer synthetic-pool")))
        .unwrap();
    assert_eq!(
        runtime.client_reasoning_levels(&key, "test", WireApi::Responses),
        ["low"]
    );
    assert_eq!(
        runtime.client_reasoning_levels(&key, "test", WireApi::Messages),
        ["low", "xhigh"]
    );
    runtime.update_key_scope(
        "key",
        CandidateScope {
            source_ids: Some(BTreeSet::from(["native".into()])),
            account_ids: None,
            ..CandidateScope::default()
        },
    );
    assert_eq!(
        runtime.client_reasoning_levels(&key, "test", WireApi::Responses),
        ["low", "xhigh"]
    );
    assert!(runtime
        .client_reasoning_levels(&key, "missing", WireApi::Responses)
        .is_empty());
}

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
                policy_model: "test",
                allowed_protocols: &WireApi::ALL,
                request_scope: &CandidateScope::default(),
                retry_at_ms: 1000,
                reason,
                now_ms: 100,
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

struct NeverRefresh;

impl TokenRefreshAdapter for NeverRefresh {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        _refresh_token: &'a str,
        _now_ms: u64,
    ) -> BoxFuture<'a, std::result::Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async {
            Err(TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                "not_called",
            ))
        })
    }
}

struct NoopPersistence;

impl TokenPersistenceAdapter for NoopPersistence {
    fn persist<'a>(
        &'a self,
        _account_id: &'a str,
        _tokens: &'a TokenSet,
    ) -> BoxFuture<'a, std::result::Result<(), TokenPersistenceFailure>> {
        Box::pin(async { Ok(()) })
    }

    fn persist_auth_state<'a>(
        &'a self,
        _account_id: &'a str,
        _auth_state: AccountAuthState,
    ) -> BoxFuture<'a, std::result::Result<(), TokenPersistenceFailure>> {
        Box::pin(async { Ok(()) })
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        _account_id: &'a str,
        _expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, std::result::Result<String, TokenPersistenceFailure>> {
        Box::pin(async move { Ok(task_id.to_string()) })
    }
}

fn source(id: &str, key: &str, models: &[&str]) -> ProviderSource {
    ProviderSource {
        id: id.to_string(),
        name: id.to_string(),
        base_url: "https://example.test/v1".to_string(),
        api_key: key.to_string(),
        wire_api: WireApi::Responses,
        models: models.iter().map(|model| (*model).to_string()).collect(),
    }
}

fn key(id: &str, secret: &str) -> LocalGatewayKey {
    LocalGatewayKey {
        id: id.to_string(),
        secret: secret.to_string(),
    }
}

#[test]
fn websocket_transport_capability_is_model_scoped_and_expires() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "provider",
            &["model-a"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    runtime.mark_websocket_http_only("source", "model-a", 1_000);
    assert!(runtime.websocket_is_http_only("source", "model-a", 1_001));
    assert!(!runtime.websocket_is_http_only("source", "model-b", 1_001));
    assert!(!runtime.websocket_is_http_only(
        "source",
        "model-a",
        1_000 + WEBSOCKET_CAPABILITY_TTL_MS
    ));
    runtime.mark_websocket_supported("source", "model-a");
    assert!(!runtime.websocket_is_http_only("source", "model-a", 1_001));
}

#[test]
fn messages_source_models_are_automatically_available_to_responses_clients() {
    let mut provider = source("anthropic-source", "provider-secret", &["claude-test"]);
    provider.wire_api = WireApi::Messages;
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(provider)],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    assert_eq!(
        runtime
            .visible_models_for_secret("local-secret", &[WireApi::Responses], current_time_ms(),),
        ["claude-test"]
    );
    assert_eq!(
        runtime.visible_models_for_secret("local-secret", &[WireApi::Messages], current_time_ms(),),
        ["claude-test"]
    );

    let route_ids = runtime
        .candidate_runtime_order()
        .into_iter()
        .map(|route| route.candidate_id)
        .collect::<Vec<_>>();
    assert!(route_ids.iter().any(|id| id == "anthropic-source"));
    assert!(route_ids
        .iter()
        .any(|id| id == "anthropic-source::responses_to_messages"));
}

fn quota_account(snapshot: QuotaSnapshot) -> RuntimeChatGptAccount {
    RuntimeChatGptAccount {
        id: "account-1".to_string(),
        source_id: "openai-codex".to_string(),
        chatgpt_account_id: "account-1".to_string(),
        responses_url: "https://example.test/v1/responses".to_string(),
        models: vec!["gpt-test".to_string()],
        enabled: true,
        draining: false,
        priority: 0,
        weight: 1,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        health: CandidateHealth::Healthy,
        quota: CandidateQuota::from_snapshot(&snapshot, 1_000, QUOTA_STALE_AFTER_MS),
        quota_updated_at_ms: snapshot.updated_at_ms,
        quota_snapshot: snapshot,
        subscription_plan_type: None,
        subscription_expires_at_ms: None,
        last_used_at_ms: None,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        proxy: None,
    }
}

fn quota_runtime(snapshot: QuotaSnapshot) -> GatewayRuntime {
    GatewayRuntime::from_mixed_pool(
        Vec::new(),
        vec![quota_account(snapshot)],
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
    .unwrap()
}

#[test]
fn automatic_responses_lite_requires_every_configured_route_to_confirm_support() {
    let account_only = quota_runtime(QuotaSnapshot::default());
    let account_only_key = account_only
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    assert!(
        !account_only.codex_model_responses_routes_all_support_lite(&account_only_key, "gpt-test")
    );
    account_only.set_codex_model_uses_responses_lite("account-1", "gpt-test", true);
    assert!(
        account_only.codex_model_responses_routes_all_support_lite(&account_only_key, "gpt-test")
    );

    let first = quota_account(QuotaSnapshot::default());
    let mut second = first.clone();
    second.id = "account-2".to_string();
    second.chatgpt_account_id = "account-2".to_string();
    let all_accounts = GatewayRuntime::from_mixed_pool(
        Vec::new(),
        vec![first, second],
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
    let all_accounts_key = all_accounts
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    all_accounts.set_codex_model_uses_responses_lite("account-1", "gpt-test", true);
    assert!(
        !all_accounts.codex_model_responses_routes_all_support_lite(&all_accounts_key, "gpt-test")
    );
    all_accounts.set_codex_model_uses_responses_lite("account-2", "gpt-test", true);
    assert!(
        all_accounts.codex_model_responses_routes_all_support_lite(&all_accounts_key, "gpt-test")
    );
    assert!(all_accounts
        .codex_model_account_responses_routes_all_support_lite(&all_accounts_key, "gpt-test"));

    let mixed = GatewayRuntime::from_mixed_pool(
        vec![RuntimeSource::unrestricted(source(
            "api-source",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![quota_account(QuotaSnapshot::default())],
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
    let mixed_key = mixed
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    mixed.set_codex_model_uses_responses_lite("account-1", "gpt-test", true);
    assert!(!mixed.codex_model_responses_routes_all_support_lite(&mixed_key, "gpt-test"));
    assert!(mixed.codex_model_account_responses_routes_all_support_lite(&mixed_key, "gpt-test"));
}

#[tokio::test]
async fn persistent_candidate_wait_does_not_expire_on_elapsed_retry_at() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        runtime.wait_for_candidate_availability(
            Some(0),
            std::time::Duration::from_millis(10),
            None,
        ),
    )
    .await
    .expect("persistent wait should use bounded backoff");

    assert!(result);
}

#[tokio::test]
async fn pool_rotation_capacity_waits_wake_without_rebuilding_the_runtime() {
    let mut policy = crate::resolve_pool_routing(
        None,
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
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
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
    assert!(runtime
        .set_pool_routing_policy(policy.clone(), 0, 3, true)
        .is_err());
    assert!(futures_util::poll!(&mut waiting).is_pending());
    runtime.set_pool_routing_policy(policy, 3, 3, true).unwrap();
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
async fn bounded_candidate_wait_retries_when_the_cooldown_elapsed_before_waiting() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    let result = tokio::time::timeout(
        std::time::Duration::from_millis(100),
        runtime.wait_for_candidate_availability(
            Some(0),
            std::time::Duration::from_millis(10),
            Some(tokio::time::Instant::now() + std::time::Duration::from_secs(1)),
        ),
    )
    .await
    .expect("an elapsed cooldown should be retried immediately");

    assert!(result);
}

#[tokio::test]
async fn recovery_probe_waits_for_its_owner_and_snapshots_match_activity_versions() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    let key = runtime.authenticated_key(&runtime.keys[0]);
    let tried = HashSet::new();
    let now = current_time_ms();
    runtime.set_candidate_cooldown("account-1", "*", now - 1);
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

#[derive(Default)]
struct RecordedResponseAffinityStore {
    found: Mutex<Vec<String>>,
    restored_binding: Mutex<Option<ResponseAffinityBinding>>,
    upserts: Mutex<Vec<ResponseAffinityBinding>>,
    deletes: Mutex<Vec<String>>,
}

impl ResponseAffinityStore for RecordedResponseAffinityStore {
    fn load(&self, _now_ms: u64) -> std::result::Result<Vec<ResponseAffinityBinding>, String> {
        Ok(Vec::new())
    }

    fn find(
        &self,
        key: &str,
        _now_ms: u64,
    ) -> std::result::Result<Option<ResponseAffinityBinding>, String> {
        self.found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(key.to_string());
        Ok(self
            .restored_binding
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone())
    }

    fn upsert(&self, binding: &ResponseAffinityBinding) -> std::result::Result<(), String> {
        self.upserts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(binding.clone());
        Ok(())
    }

    fn delete(&self, key: &str) -> std::result::Result<(), String> {
        self.deletes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(key.to_string());
        Ok(())
    }

    fn delete_candidate(&self, _candidate_id: &str) -> std::result::Result<(), String> {
        Ok(())
    }
}

#[test]
fn response_affinity_persists_and_removes_the_same_scheduler_binding() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();

    let response_id = "resp-1";
    let affinity_key = runtime.response_affinity_key(Some(response_id)).unwrap();
    runtime.bind_response_affinity(Some(response_id), "source-1", 123);

    assert_eq!(
        *store
            .upserts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![ResponseAffinityBinding {
            key: affinity_key.clone(),
            candidate_id: "source-1".to_string(),
            expires_at_ms: 123 + crate::RESPONSE_AFFINITY_TTL_MS,
        }]
    );
    assert!(runtime.invalidate_response_affinity(Some(&affinity_key)));
    assert!(!runtime.invalidate_response_affinity(Some(&affinity_key)));
    assert_eq!(
        *store
            .deletes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![affinity_key]
    );
}

#[tokio::test]
async fn incomplete_response_affinity_is_connection_scoped_and_never_persisted() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "secret-a",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..Default::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let normal_key = runtime.response_affinity_key(Some("resp_partial")).unwrap();
    let connection_key = runtime
        .bind_volatile_response_affinity(Some("resp_partial"), "source-a", "request-1", 123)
        .unwrap();
    assert!(!runtime.has_response_affinity_binding(&normal_key, 123));
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (Some(&connection_key), None),
            124,
        )
        .await
        .unwrap();
    assert!(selection.response_affinity_hit);
    drop(lease);
    assert!(store.upserts.lock().unwrap().is_empty());
    assert!(runtime.invalidate_response_affinity(Some(&connection_key)));
    assert!(!runtime.has_response_affinity_binding(&normal_key, 125));
}

#[tokio::test]
async fn saving_bridge_continuation_binds_its_response_to_the_creating_candidate() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "secret-a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "secret-b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let bridge_request = crate::prepare_responses_to_messages_scoped(
        &serde_json::json!({"model": "gpt-test", "input": "hello"}),
        "gpt-test",
        false,
        MessagesReasoningMode::Disabled,
        None,
        "source-a",
    )
    .unwrap();
    let bridge_response = crate::protocol::translate_messages_response(
        bridge_request,
        &serde_json::json!({
            "id": "msg-1",
            "content": [{"type": "text", "text": "hello"}]
        }),
    )
    .unwrap();

    runtime.save_messages_bridge_response("key-1", "source-a", &bridge_response, 123);

    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let affinity_key = runtime
        .response_affinity_key(Some(&bridge_response.response_id))
        .unwrap();
    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (Some(&affinity_key), None),
            124,
        )
        .await
        .unwrap();

    assert_eq!(selection.candidate_id, "source-a");
    assert!(selection.response_affinity_hit);
    assert_eq!(
        selection.diagnostics.reason,
        crate::SelectionReason::ResponseAffinity
    );
    drop(lease);
}

#[test]
fn prompt_affinity_persists_only_its_opaque_binding_and_ttl() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();

    runtime.bind_prompt_affinity(Some("cache:opaque-hash"), "source-1", 123);

    assert_eq!(
        *store
            .upserts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![ResponseAffinityBinding {
            key: "cache:opaque-hash".to_string(),
            candidate_id: "source-1".to_string(),
            expires_at_ms: 123 + crate::PROMPT_AFFINITY_TTL_MS,
        }]
    );
}

#[test]
fn prompt_affinity_uses_explicit_cache_key_before_session_context() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    let explicit = runtime.prompt_affinity_key(
        "key-1",
        "gpt-test",
        Some("cache-key"),
        Some("client-session"),
    );
    let session = runtime.prompt_affinity_key("key-1", "gpt-test", None, Some("client-session"));
    let other_session =
        runtime.prompt_affinity_key("key-1", "gpt-test", None, Some("other-session"));

    assert!(explicit.is_some());
    assert!(session.is_some());
    assert_ne!(explicit, session);
    assert_ne!(session, other_session);
    assert_eq!(
        session,
        runtime.prompt_affinity_key("key-1", "gpt-test", None, Some("client-session"))
    );
    assert!(runtime
        .prompt_affinity_key("key-1", "gpt-test", None, None)
        .is_none());
}

#[tokio::test]
async fn selection_restores_persisted_response_affinity_before_reserving() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "secret-a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "secret-b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let response_id = "resp-restored";
    let affinity_key = runtime.response_affinity_key(Some(response_id)).unwrap();
    *store
        .restored_binding
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ResponseAffinityBinding {
        key: affinity_key.clone(),
        candidate_id: "source-b".to_string(),
        expires_at_ms: 123 + crate::RESPONSE_AFFINITY_TTL_MS,
    });
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();

    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (Some(&affinity_key), None),
            123,
        )
        .await
        .unwrap();

    assert_eq!(selection.candidate_id, "source-b");
    assert!(selection.response_affinity_hit);
    assert_eq!(
        *store
            .found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![affinity_key.clone()]
    );
    assert_eq!(
        *store
            .upserts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![ResponseAffinityBinding {
            key: affinity_key,
            candidate_id: "source-b".to_string(),
            expires_at_ms: 123 + crate::RESPONSE_AFFINITY_TTL_MS,
        }]
    );
    drop(lease);
}

#[tokio::test]
async fn selection_restores_persisted_prompt_affinity_before_reserving() {
    let store = Arc::new(RecordedResponseAffinityStore::default());
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "secret-a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "secret-b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            response_affinity_store: Some(store.clone()),
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    assert!(runtime.update_candidate_availability_at(
        "source-a",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Available(6_500),
        Some(123),
    ));
    assert!(runtime.update_candidate_availability_at(
        "source-b",
        true,
        CandidateHealth::Healthy,
        CandidateQuota::Available(9_000),
        Some(123),
    ));
    let affinity_key = "cache:restored-prompt";
    *store
        .restored_binding
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(ResponseAffinityBinding {
        key: affinity_key.to_string(),
        candidate_id: "source-a".to_string(),
        expires_at_ms: 123 + crate::PROMPT_AFFINITY_TTL_MS,
    });
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();

    let (selection, lease) = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (None, Some(affinity_key)),
            123,
        )
        .await
        .unwrap();

    assert_eq!(selection.candidate_id, "source-a");
    assert_eq!(
        selection.diagnostics.reason,
        crate::SelectionReason::PromptCacheAffinity
    );
    assert_eq!(
        *store
            .found
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        vec![affinity_key.to_string()]
    );
    drop(lease);
}

#[test]
fn runtime_updates_service_tier_and_removes_candidates_in_place() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    assert_eq!(runtime.default_service_tier(), DefaultServiceTier::Standard);
    runtime.set_default_service_tier(DefaultServiceTier::Fast);
    assert_eq!(runtime.default_service_tier(), DefaultServiceTier::Fast);
    assert!(runtime.remove_candidate("source-1"));
    assert!(runtime.candidate_runtime_order().is_empty());
}

#[test]
fn service_tier_storage_values_keep_fast_aliases_compatible() {
    assert_eq!(DefaultServiceTier::Standard.as_str(), "standard");
    assert_eq!(DefaultServiceTier::Fast.as_str(), "fast");
    assert_eq!(
        DefaultServiceTier::from_storage_value("priority"),
        DefaultServiceTier::Fast
    );
    assert_eq!(
        DefaultServiceTier::from_storage_value("unknown"),
        DefaultServiceTier::Standard
    );
}

#[test]
fn runtime_updates_source_policy_without_rebuilding_candidate_state() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["model-a", "model-b"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let retry_at = current_time_ms() + 60_000;
    runtime.set_candidate_cooldown("source-1", "model-a", retry_at);

    assert!(runtime.update_source_policy(
        "source-1",
        RuntimeCandidatePolicy {
            enabled: true,
            draining: false,
            priority: 7,
            weight: 3,
            allowed_models: vec!["model-b".into()],
            excluded_models: Vec::new(),
        },
        30,
    ));
    assert_eq!(
        runtime.visible_models_for_secret("local-secret", &[WireApi::Responses], current_time_ms()),
        vec!["model-b"]
    );
    let candidate = runtime
        .lock_scheduler()
        .candidate("source-1")
        .cloned()
        .unwrap();
    assert_eq!(candidate.priority, 7);
    assert_eq!(candidate.weight, 3);
    assert_eq!(candidate.cooldowns.get("model-a"), Some(&retry_at));
    assert_eq!(runtime.source_recovery_delay_ms("source-1"), Some(30_000));
}

#[test]
fn runtime_rejects_policy_updates_for_missing_candidates() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["model-a"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let policy = RuntimeCandidatePolicy {
        enabled: true,
        draining: false,
        priority: 7,
        weight: 3,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
    };

    assert!(!runtime.update_source_policies(&[
        RuntimeSourcePolicyUpdate {
            source_id: "source-1".into(),
            policy: policy.clone(),
            recovery_delay_seconds: 30,
        },
        RuntimeSourcePolicyUpdate {
            source_id: "missing".into(),
            policy: policy.clone(),
            recovery_delay_seconds: 30,
        },
    ]));
    assert_eq!(
        runtime
            .lock_scheduler()
            .candidate("source-1")
            .expect("source candidate")
            .priority,
        0
    );
    assert!(!runtime.update_account_policy("missing", policy));
}

#[test]
fn runtime_updates_key_scope_without_rebuild() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "a", &["model-a"])),
            RuntimeSource::unrestricted(source("source-b", "b", &["model-b"])),
        ],
        vec![RuntimeLocalKey {
            key: key("key-1", "local-secret"),
            enabled: true,
            source_ids: Some(vec!["source-a".into()]),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
        }],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    assert_eq!(
        runtime.visible_models_for_secret("local-secret", &[WireApi::Responses], current_time_ms()),
        vec!["model-a"]
    );
    assert!(runtime.update_key_scope(
        "key-1",
        CandidateScope {
            source_ids: Some(std::iter::once("source-b".to_string()).collect()),
            account_ids: Some(Default::default()),
            model_rules: ModelRules::default(),
        },
    ));
    assert_eq!(
        runtime.visible_models_for_secret("local-secret", &[WireApi::Responses], current_time_ms()),
        vec!["model-b"]
    );
}

#[test]
fn response_affinity_owner_tracks_live_key_scope() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey {
            key: key("key-1", "local-secret"),
            enabled: true,
            source_ids: Some(vec!["source-a".into()]),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
        }],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    runtime.bind_response_affinity(Some("resp-1"), "source-a", 1);
    let affinity_key = runtime.response_affinity_key(Some("resp-1")).unwrap();

    assert_eq!(
        runtime.response_affinity_owner_supports_route(
            &authenticated,
            &affinity_key,
            "gpt-test",
            &[WireApi::Responses],
            2,
        ),
        Some(true)
    );
    assert!(runtime.update_key_scope(
        "key-1",
        CandidateScope {
            source_ids: Some(BTreeSet::from(["source-b".to_string()])),
            ..CandidateScope::default()
        },
    ));
    assert_eq!(
        runtime.response_affinity_owner_supports_route(
            &authenticated,
            &affinity_key,
            "gpt-test",
            &[WireApi::Responses],
            2,
        ),
        Some(false),
        "removing a provider from the key scope must release its chat affinity"
    );
}

#[test]
fn optional_response_affinity_is_released_when_owner_needs_reauthentication() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "a", &["gpt-test"])),
            RuntimeSource::unrestricted(source("source-b", "b", &["gpt-test"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    runtime.bind_response_affinity(Some("resp-1"), "source-a", 1);
    let affinity_key = runtime.response_affinity_key(Some("resp-1")).unwrap();
    assert!(runtime.set_candidate_health("source-a", CandidateHealth::ReauthRequired));
    assert_eq!(
        runtime.response_affinity_owner_supports_route(
            &authenticated,
            &affinity_key,
            "gpt-test",
            &[WireApi::Responses],
            2,
        ),
        Some(true)
    );
    assert_eq!(
        runtime.response_affinity_owner_is_eligible(
            &authenticated,
            &affinity_key,
            "gpt-test",
            &[WireApi::Responses],
            2,
        ),
        Some(false)
    );

    let mut optional_affinity = Some(affinity_key.clone());
    assert!(runtime.release_unroutable_response_affinity(
        &authenticated,
        &mut optional_affinity,
        "gpt-test",
        &[WireApi::Responses],
        2,
    ));
    assert!(optional_affinity.is_none());
    assert!(runtime.has_response_affinity_binding(&affinity_key, 2));
    assert_eq!(
        runtime
            .response_affinity_candidate(&affinity_key, 2)
            .as_deref(),
        Some("source-a")
    );
}

#[test]
fn active_responses_scope_uses_live_candidate_policy() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "upstream-secret",
            &["model-a"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let mut account = runtime
        .lock_scheduler()
        .candidate("source-a")
        .cloned()
        .unwrap();
    account.id = "account-a".into();
    account.kind = CandidateKind::OAuthAccount;
    account.source_id = "codex".into();
    account.account_id = Some("account-a".into());
    runtime.lock_scheduler().upsert(account);

    let source_ids = BTreeSet::from(["source-a".to_string()]);
    let account_ids = BTreeSet::from(["account-a".to_string()]);
    assert_eq!(
        runtime.active_responses_scope(&source_ids, &account_ids),
        CandidateScope {
            source_ids: Some(source_ids.clone()),
            account_ids: Some(account_ids.clone()),
            model_rules: ModelRules::default(),
        }
    );

    assert!(runtime.update_source_policy(
        "source-a",
        RuntimeCandidatePolicy {
            enabled: false,
            draining: false,
            priority: 0,
            weight: 1,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
        },
        0,
    ));
    assert!(runtime.update_account_policy(
        "account-a",
        RuntimeCandidatePolicy {
            enabled: false,
            draining: false,
            priority: 0,
            weight: 1,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
        },
    ));
    let scope = runtime.active_responses_scope(&source_ids, &account_ids);
    assert_eq!(scope.source_ids, Some(BTreeSet::new()));
    assert_eq!(scope.account_ids, Some(BTreeSet::new()));
}

#[tokio::test]
async fn source_capability_failure_does_not_permanently_hide_a_declared_model() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();
    runtime.apply_usage_event(
        &UsageEvent {
            request_id: "request".into(),
            attempt: 1,
            local_key_id: "key-1".into(),
            source_id: "source-1".into(),
            candidate_id: Some("source-1".into()),
            account_id: None,
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
            http_status: StatusCode::BAD_REQUEST.as_u16(),
            error_category: Some("upstream_model_not_found".into()),
            tool_use: ToolUseDiagnostics::default(),
            cooldown_scope: Some("gpt-test".into()),
            retry_at_ms: Some(now_ms.saturating_add(60_000)),
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
        now_ms,
    );

    let selection = runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (None, None),
            now_ms,
        )
        .await;
    assert!(selection.is_some());
}

#[tokio::test]
async fn direct_source_rate_limit_uses_the_shared_model_storm_breaker() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions {
            provider_storm_breaker: true,
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();

    assert!(!runtime.record_provider_rate_limit("source-1", "gpt-test", 123));
    assert!(!runtime.record_provider_rate_limit("source-1", "gpt-test", 124));
    assert!(runtime.record_provider_rate_limit("source-1", "gpt-test", 125));
    assert!(runtime
        .select_and_reserve(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            &HashSet::new(),
            (None, None),
            125,
        )
        .await
        .is_none());
}

#[test]
fn source_connector_preserves_normalized_binding_and_model_order() {
    let source = source(
        "source-1",
        "upstream-secret",
        &[
            "gpt-5.6-sol",
            "claude-opus-5",
            "gpt-5.4-mini",
            "claude-sonnet-5",
        ],
    );
    let bindings = normalize_source_protocol_bindings(
        vec![
            SourceProtocolBinding {
                wire_api: WireApi::Messages,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["claude-opus-5".into(), "claude-sonnet-5".into()],
            },
            SourceProtocolBinding {
                wire_api: WireApi::Responses,
                adapter: SourceAdapter::Native,
                reasoning_mode: MessagesReasoningMode::Disabled,
                cache_write_ttl: Default::default(),
                model_ids: vec!["gpt-5.6-sol".into(), "gpt-5.4-mini".into()],
            },
        ],
        source.wire_api,
        &source.models,
    )
    .unwrap();

    let connector = SourceConnector::new(&source, &bindings).unwrap();

    assert_eq!(connector.protocol_bindings(), bindings.as_slice());
    assert_eq!(
        connector
            .canonical_model_for(bindings[1].key(), "GPT-5.4-MINI")
            .as_deref(),
        Some("gpt-5.4-mini")
    );
    assert!(!connector
        .protocol_bindings()
        .iter()
        .any(|binding| binding.wire_api == WireApi::ChatCompletions));
}

#[test]
fn speed_choices_and_preferences_are_independent_of_source_metadata_and_health() {
    let model = "gpt-future-synthetic";
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-1", "synthetic-one", &[model])),
            RuntimeSource::unrestricted(source("source-2", "synthetic-two", &[model])),
        ],
        vec![RuntimeLocalKey::unrestricted(key(
            "key-1",
            "synthetic-pool",
        ))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let tiers = [
        DefaultServiceTier::Standard,
        DefaultServiceTier::Fast,
        DefaultServiceTier::Ultrafast,
    ];
    assert_eq!(runtime.model_supported_service_tiers(model), tiers);
    runtime
        .set_model_service_tier_overrides(BTreeMap::from([(
            model.into(),
            DefaultServiceTier::Ultrafast,
        )]))
        .unwrap();
    runtime.set_candidate_cooldown("source-1", model, current_time_ms() + 60_000);
    runtime.remove_candidate("source-2");
    assert_eq!(
        runtime.model_effective_service_tier(model),
        DefaultServiceTier::Ultrafast
    );
    assert_eq!(runtime.model_supported_service_tiers(model), tiers);
    assert_eq!(
        runtime.model_supported_service_tiers("claude-synthetic"),
        [DefaultServiceTier::Standard]
    );
}

#[test]
fn service_tier_normalization_preserves_valid_ids_for_model_policy() {
    let normalized = normalize_model_service_tier_overrides(BTreeMap::from([
        ("provider/gpt-5".to_string(), DefaultServiceTier::Fast),
        ("provider/claude-5".to_string(), DefaultServiceTier::Fast),
    ]))
    .unwrap();

    assert_eq!(
        normalized,
        BTreeMap::from([
            ("provider/gpt-5".to_string(), DefaultServiceTier::Fast),
            ("provider/claude-5".to_string(), DefaultServiceTier::Fast),
        ])
    );
}

#[test]
fn image_main_model_prefers_cheapest_tier_without_model_name_allowlist() {
    let models = normalized_set(
        [
            "gpt-5.6-terra".to_string(),
            "gpt-5.6-sol".to_string(),
            "gpt-5.4-mini".to_string(),
        ]
        .iter()
        .collect::<Vec<_>>(),
    );
    // Automatic selection uses the immutable LiteLLM snapshot when one is
    // available.  Keep the fixture explicit so this test does not depend on
    // the shared LiteLLM fixture catalog.
    let catalog = crate::pricing::PricingCatalog::from_litellm_json(
        r#"{
            "gpt-5.6-terra": {
                "litellm_provider": "openai",
                "input_cost_per_token": "0.000002",
                "output_cost_per_token": "0.000012"
            },
            "gpt-5.6-sol": {
                "litellm_provider": "openai",
                "input_cost_per_token": "0.000004",
                "output_cost_per_token": "0.000020"
            },
            "gpt-5.4-mini": {
                "litellm_provider": "openai",
                "input_cost_per_token": "0.000001",
                "output_cost_per_token": "0.000006"
            }
        }"#,
    )
    .unwrap();
    assert_eq!(
        super::images::cheapest_image_main_model_with_catalog(&models, Some(&catalog)).as_deref(),
        Some("gpt-5.4-mini")
    );
    // An empty/offline snapshot must still allow a deterministic runtime
    // build; its choice is a stable fallback, not an implicit price claim.
    let empty = crate::pricing::PricingCatalog::empty();
    assert_eq!(
        super::images::cheapest_image_main_model_with_catalog(&models, Some(&empty)).as_deref(),
        Some("gpt-5.6-sol")
    );
    let terra = normalized_set(["gpt-5.6-terra".to_string()].iter());
    assert_eq!(
        cheapest_image_main_model(&terra).as_deref(),
        Some("gpt-5.6-terra")
    );
    let image = normalized_set([IMAGE_API_MODEL.to_string()].iter());
    assert!(cheapest_image_main_model(&image).is_none());
}

#[test]
fn explicit_image_base_model_is_used_only_when_available() {
    let models = normalized_set(
        ["gpt-5.4-mini".to_string(), "gpt-5.6-sol".to_string()]
            .iter()
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        select_image_main_model(&models, Some("gpt-5.6-sol")).as_deref(),
        Some("gpt-5.6-sol")
    );
    assert_eq!(select_image_main_model(&models, Some("future-model")), None);
    let future = normalized_set(["gpt-future".to_string()].iter());
    assert!(cheapest_image_main_model(&future).is_none());
    assert_eq!(
        select_image_main_model(&future, Some("gpt-future")).as_deref(),
        Some("gpt-future")
    );
    let legacy = normalized_set(["gpt-4.1-mini".to_string()].iter());
    assert!(cheapest_image_main_model(&legacy).is_none());
    assert_eq!(
        normalize_image_base_model(Some(" auto ".into())).unwrap(),
        None
    );
}

#[test]
fn local_auth_returns_only_the_matching_redacted_key_policy() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![
            RuntimeLocalKey::unrestricted(key("key-1", "local-secret")),
            RuntimeLocalKey::unrestricted(key("key-2", "other-secret")),
        ],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    assert_eq!(authenticated.id, "key-1");
    assert!(runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer upstream-secret")))
        .is_none());
    assert!(!format!("{runtime:?}").contains("local-secret"));
    assert!(!format!("{runtime:?}").contains("upstream-secret"));
}

#[test]
fn key_scope_and_prefix_filter_visible_models_without_scope_escalation() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "a", &["gpt-a"])),
            RuntimeSource::unrestricted(source("source-b", "b", &["gpt-b"])),
        ],
        vec![RuntimeLocalKey {
            key: key("key", "secret"),
            enabled: true,
            source_ids: Some(vec!["source-a".into()]),
            allowed_models: vec!["gpt-*".into()],
            excluded_models: vec!["gpt-b".into()],
            model_prefix: Some("team".into()),
        }],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    assert_eq!(
        runtime.visible_models(&authenticated, &[WireApi::Responses], current_time_ms()),
        vec!["team/gpt-a"]
    );
    assert_eq!(
        runtime.visible_models_for_secret("secret", &[WireApi::Responses], current_time_ms()),
        vec!["team/gpt-a"]
    );
    assert!(runtime
        .visible_models_for_secret("wrong", &[WireApi::Responses], current_time_ms())
        .is_empty());
    assert_eq!(
        runtime
            .resolve_model(&authenticated, "TEAM/gpt-a")
            .as_deref(),
        Some("gpt-a")
    );
}

#[test]
fn codex_aliases_resolve_without_shadowing_exact_model_ids() {
    let encoded = crate::codex_model_alias("vendor/model");
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "upstream-secret",
            &["vendor/model", &encoded],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();

    assert_eq!(
        runtime
            .resolve_visible_model(
                &authenticated,
                &encoded,
                &[WireApi::Responses],
                current_time_ms(),
            )
            .as_deref(),
        Some(encoded.as_str())
    );

    let alias = crate::codex_model_alias("vendor/model");
    let without_collision = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "upstream-secret",
            &["vendor/model"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = without_collision
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    assert_eq!(
        without_collision
            .resolve_visible_model(
                &authenticated,
                &alias,
                &[WireApi::Responses],
                current_time_ms(),
            )
            .as_deref(),
        Some("vendor/model")
    );
}

#[test]
fn configured_model_resolution_accepts_temporary_health_outage_but_not_unknown_models() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source",
            "upstream-secret",
            &["gpt-test"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();
    assert!(runtime.set_candidate_health("source", CandidateHealth::Unhealthy));

    assert!(runtime
        .resolve_visible_model(
            &authenticated,
            "gpt-test",
            &[WireApi::Responses],
            current_time_ms(),
        )
        .is_none());
    assert_eq!(
        runtime
            .resolve_configured_model(&authenticated, "gpt-test", &[WireApi::Responses])
            .as_deref(),
        Some("gpt-test")
    );
    assert!(runtime
        .resolve_configured_model(&authenticated, "unknown", &[WireApi::Responses])
        .is_none());
}

#[test]
fn explicit_empty_scope_cannot_start_a_gateway() {
    let error = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "a",
            &["gpt-a"],
        ))],
        vec![RuntimeLocalKey {
            key: key("key", "secret"),
            enabled: true,
            source_ids: Some(Vec::new()),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
        }],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("no enabled gateway credential"));
}

#[test]
fn transition_runtime_allows_an_explicitly_empty_scope() {
    let runtime = GatewayRuntime::build(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "a",
            &["gpt-a"],
        ))],
        Vec::new(),
        vec![RuntimeMixedLocalKey {
            key: key("key", "secret"),
            enabled: true,
            source_ids: Some(Vec::new()),
            account_ids: None,
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            model_prefix: None,
            wire_apis: None,
        }],
        None,
        ReachabilityRequirement::AllowUnroutable,
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();

    assert!(runtime
        .visible_models_for_secret("secret", &[WireApi::Responses], current_time_ms())
        .is_empty());
}

#[test]
fn global_hidden_models_apply_to_listing_and_requests() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-a",
            "a",
            &["gpt-new", "gpt-old"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key", "secret"))],
        GatewayRuntimeOptions {
            hidden_models: vec!["GPT-OLD".into()],
            ..GatewayRuntimeOptions::default()
        },
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer secret")))
        .unwrap();

    assert_eq!(
        runtime.visible_models(&authenticated, &[WireApi::Responses], current_time_ms()),
        vec!["gpt-new"]
    );
    assert!(runtime.resolve_model(&authenticated, "gpt-old").is_none());
}
