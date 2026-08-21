use super::*;
use crate::accounts::{
    AccountAuthState, TokenPersistenceAdapter, TokenPersistenceFailure, TokenRefresh,
    TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
};
use crate::catalog::source_reasoning_capabilities;
use crate::{CandidateHealth, CandidateQuota, ToolUseDiagnostics, QUOTA_STALE_AFTER_MS};
use futures_util::future::BoxFuture;
use std::collections::HashMap;

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
fn messages_source_models_are_available_to_responses_and_messages_clients() {
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
    assert!(route_ids
        .iter()
        .any(|id| id == "anthropic-source::messages"));
    assert!(route_ids
        .iter()
        .any(|id| id == "anthropic-source::responses_to_messages"));
}

fn confirmed_reasoning_probe() -> serde_json::Value {
    serde_json::json!({
        "status": "confirmed",
        "total": 8,
        "running": 0,
        "success": 3,
        "failed": 5,
        "confirmed": 3,
        "rejected": 5,
        "inconclusive": 0,
        "pending": 0,
        "lastProbeAt": "2026-08-19T00:00:00Z"
    })
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

#[tokio::test]
async fn generic_source_reasoning_metadata_survives_a_codex_catalog_cache_update() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["provider/fable"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();

    runtime.remember_source_model_manifest(
        "source-1",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "context_window": 1_000_000,
                "reasoningEffortModes": ["low", "high"],
                "defaultReasoningLevel": "high",
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );
    // Codex asks the same source for a different payload shape. It must
    // not overwrite the generic source metadata that powers the selector.
    runtime.remember_codex_model_manifest(
        "source-1",
        serde_json::json!({"models": [{"slug": "provider/fable"}]}),
        now_ms,
    );

    let metadata = runtime
        .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
        .await;

    assert_eq!(
        metadata.context_windows,
        BTreeMap::from([("provider/fable".to_string(), 1_000_000)])
    );
    assert_eq!(
        metadata.reasoning_catalog_templates["provider/fable"],
        serde_json::json!({
            "supported_reasoning_levels": [
                {"effort": "low", "description": "low"},
                {"effort": "high", "description": "high"}
            ]
        })
        .as_object()
        .unwrap()
        .clone()
    );
}

#[tokio::test]
async fn fresh_source_metadata_does_not_wait_for_another_refresh() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["provider/fable"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();
    runtime.remember_source_model_manifest(
        "source-1",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low", "high"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );

    let guard = runtime.model_metadata.refresh_lock.lock().await;
    let metadata = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        runtime.codex_source_model_metadata(&key, &[WireApi::Responses], now_ms),
    )
    .await
    .expect("fresh metadata must not wait for the refresh lock");
    drop(guard);

    assert!(metadata
        .reasoning_catalog_templates
        .contains_key("provider/fable"));
}

#[tokio::test]
async fn management_prefetch_populates_reasoning_before_codex_catalog_request() {
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(source(
                "source-1",
                "upstream-secret",
                &["provider/fable"],
            ))],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap(),
    );
    runtime.remember_source_model_manifest(
        "source-1",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low", "medium", "high"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        current_time_ms(),
    );

    runtime.prefetch_source_model_metadata();
    tokio::time::timeout(Duration::from_secs(1), async {
        while runtime
            .confirmed_source_reasoning_levels("provider/fable")
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("management prefetch completes");

    assert_eq!(
        runtime.confirmed_source_reasoning_levels("provider/fable"),
        vec!["low".to_string(), "medium".to_string(), "high".to_string()]
    );
    let not_before = runtime
        .model_metadata
        .prefetch_not_before_ms
        .load(Ordering::Acquire);
    assert!(not_before > runtime_now_ms());
    runtime.prefetch_source_model_metadata();
    assert_eq!(
        runtime
            .model_metadata
            .prefetch_not_before_ms
            .load(Ordering::Acquire),
        not_before
    );
}

#[tokio::test]
async fn management_prefetch_reports_chat_completions_reasoning_without_enabling_it() {
    let mut chat_source = source("chat-source", "upstream-secret", &["provider/fable"]);
    chat_source.wire_api = WireApi::ChatCompletions;
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(chat_source)],
            vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap(),
    );
    runtime.remember_source_model_manifest(
        "chat-source",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["high"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        current_time_ms(),
    );

    runtime.prefetch_source_model_metadata();
    tokio::time::timeout(Duration::from_secs(1), async {
        while runtime
            .confirmed_source_reasoning_levels("provider/fable")
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("management prefetch reports the Chat Completions route");

    assert!(!runtime.candidate_reasoning_effort_is_allowed(
        "chat-source",
        "provider/fable",
        "high"
    ));
    runtime
        .set_model_reasoning_allowed_levels(BTreeMap::from([(
            "provider/fable".to_string(),
            vec!["high".to_string()],
        )]))
        .unwrap();
    assert!(runtime.candidate_reasoning_effort_is_allowed("chat-source", "provider/fable", "high"));
}

#[tokio::test]
async fn management_prefetch_ignores_sources_outside_the_active_key_scope() {
    let runtime = Arc::new(
        GatewayRuntime::from_pool(
            vec![
                RuntimeSource::unrestricted(source(
                    "source-in-pool",
                    "in-pool-secret",
                    &["provider/fable"],
                )),
                RuntimeSource::unrestricted(source(
                    "source-outside-pool",
                    "outside-pool-secret",
                    &["provider/fable"],
                )),
            ],
            vec![RuntimeLocalKey {
                key: key("key-1", "local-secret"),
                enabled: true,
                source_ids: Some(vec!["source-in-pool".into()]),
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
            }],
            GatewayRuntimeOptions::default(),
            Arc::new(|_| {}),
        )
        .unwrap(),
    );
    let now_ms = current_time_ms();
    runtime.remember_source_model_manifest(
        "source-in-pool",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );
    runtime.remember_source_model_manifest(
        "source-outside-pool",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["ultra"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );

    runtime.prefetch_source_model_metadata();
    tokio::time::timeout(Duration::from_secs(1), async {
        while runtime
            .confirmed_source_reasoning_levels("provider/fable")
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("management prefetch completes");

    assert_eq!(
        runtime.confirmed_source_reasoning_levels("provider/fable"),
        vec!["low".to_string()]
    );
}

#[tokio::test]
async fn management_prefetch_includes_sources_for_an_account_only_key_scope() {
    let runtime = Arc::new(
        GatewayRuntime::build(
            vec![RuntimeSource::unrestricted(source(
                "source-shared",
                "shared-secret",
                &["provider/fable"],
            ))],
            Vec::new(),
            vec![RuntimeMixedLocalKey {
                key: key("key-account-only", "local-secret"),
                enabled: true,
                source_ids: None,
                account_ids: Some(vec!["account-only".into()]),
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
        .unwrap(),
    );
    let now_ms = current_time_ms();
    runtime.remember_source_model_manifest(
        "source-shared",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );

    runtime.prefetch_source_model_metadata();
    tokio::time::timeout(Duration::from_secs(1), async {
        while runtime
            .confirmed_source_reasoning_levels("provider/fable")
            .is_empty()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("an account-only key leaves source access unrestricted");

    assert_eq!(
        runtime.confirmed_source_reasoning_levels("provider/fable"),
        vec!["low".to_string()]
    );
}

#[tokio::test]
async fn scoped_catalog_refresh_keeps_reasoning_confirmed_by_another_route() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "source-a-secret", &["provider/fable"])),
            RuntimeSource::unrestricted(source("source-b", "source-b-secret", &["provider/fable"])),
        ],
        vec![
            RuntimeLocalKey::unrestricted(key("key-all", "all-secret")),
            RuntimeLocalKey {
                key: key("key-a", "source-a-only-secret"),
                enabled: true,
                source_ids: Some(vec!["source-a".to_string()]),
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                model_prefix: None,
            },
        ],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let all_key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer all-secret")))
        .unwrap();
    let source_a_key = runtime
        .authenticate(Some(&HeaderValue::from_static(
            "Bearer source-a-only-secret",
        )))
        .unwrap();
    let now_ms = current_time_ms();
    runtime.remember_source_model_manifest(
        "source-a",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );
    runtime.remember_source_model_manifest(
        "source-b",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["ultra"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );

    runtime
        .codex_source_model_metadata(&all_key, &[WireApi::Responses], now_ms)
        .await;
    assert_eq!(
        runtime.confirmed_source_reasoning_levels("provider/fable"),
        vec!["low".to_string(), "ultra".to_string()]
    );

    let scoped_metadata = runtime
        .codex_source_model_metadata(&source_a_key, &[WireApi::Responses], now_ms)
        .await;
    assert_eq!(
        scoped_metadata.reasoning_catalog_templates["provider/fable"]["supported_reasoning_levels"],
        serde_json::json!([{"effort": "low", "description": "low"}])
    );
    assert_eq!(
        runtime.confirmed_source_reasoning_levels("provider/fable"),
        vec!["low".to_string(), "ultra".to_string()]
    );
}

#[tokio::test]
async fn manually_enabled_reasoning_effort_is_scoped_to_the_model_group() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source("source-a", "source-a-secret", &["provider/fable"])),
            RuntimeSource::unrestricted(source("source-b", "source-b-secret", &["provider/fable"])),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();
    runtime.remember_source_model_manifest(
        "source-a",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low", "high"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );
    runtime.remember_source_model_manifest(
        "source-b",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );

    runtime
        .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
        .await;

    assert!(runtime.candidate_reasoning_effort_is_allowed("source-a", "provider/fable", "high"));
    assert!(!runtime.candidate_reasoning_effort_is_allowed("source-b", "provider/fable", "high"));
    runtime
        .set_model_reasoning_allowed_levels(BTreeMap::from([(
            "provider/fable".to_string(),
            vec!["high".to_string()],
        )]))
        .unwrap();
    assert!(runtime.candidate_reasoning_effort_is_allowed("source-a", "provider/fable", "high"));
    assert!(runtime.candidate_reasoning_effort_is_allowed("source-b", "provider/fable", "high"));
    assert!(!runtime.candidate_reasoning_effort_is_allowed("source-b", "provider/fable", "low"));
}

#[test]
fn manually_enabled_reasoning_skips_a_bridge_that_cannot_carry_it() {
    let mut bridged = RuntimeSource::unrestricted(source(
        "source-bridge",
        "bridge-secret",
        &["provider/fable"],
    ));
    bridged.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::ResponsesToMessages,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["provider/fable".into()],
    }];
    let runtime = GatewayRuntime::from_pool(
        vec![bridged],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    runtime
        .set_model_reasoning_allowed_levels(BTreeMap::from([(
            "provider/fable".to_string(),
            vec!["high".to_string()],
        )]))
        .unwrap();

    assert!(!runtime.candidate_reasoning_effort_is_allowed(
        "source-bridge",
        "provider/fable",
        "high"
    ));
}

#[tokio::test]
async fn stale_source_metadata_survives_a_transient_models_failure() {
    let mut unavailable_source = source("source-1", "upstream-secret", &["provider/fable"]);
    unavailable_source.base_url = "http://127.0.0.1:1/v1".to_string();
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(unavailable_source)],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();
    runtime.remember_source_model_manifest(
        "source-1",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low", "medium", "high"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms.saturating_sub(CODEX_SOURCE_MODEL_MANIFEST_TTL_MS + 1),
    );

    let metadata = runtime
        .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
        .await;

    assert!(metadata
        .reasoning_catalog_templates
        .contains_key("provider/fable"));
    assert_eq!(
        runtime.confirmed_source_reasoning_levels("provider/fable"),
        vec!["low".to_string(), "medium".to_string(), "high".to_string()]
    );
}

#[tokio::test]
async fn source_reasoning_union_keeps_unknown_route_eligible() {
    let runtime = GatewayRuntime::from_pool(
        vec![
            RuntimeSource::unrestricted(source(
                "source-confirmed",
                "confirmed-secret",
                &["provider/fable"],
            )),
            RuntimeSource::unrestricted(source(
                "source-unknown",
                "unknown-secret",
                &["provider/fable"],
            )),
        ],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();
    runtime.remember_source_model_manifest(
        "source-confirmed",
        serde_json::json!({
            "data": [{
                "id": "provider/fable",
                "reasoningEffortModes": ["low", "high"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        now_ms,
    );
    runtime.remember_source_model_manifest(
        "source-unknown",
        serde_json::json!({"data": [{"id": "provider/fable"}]}),
        now_ms,
    );

    let metadata = runtime
        .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
        .await;

    assert!(metadata
        .reasoning_catalog_templates
        .contains_key("provider/fable"));
    assert_eq!(
        runtime.confirmed_source_reasoning_levels("provider/fable"),
        vec!["low".to_string(), "high".to_string()]
    );
    assert!(runtime.model_reasoning_effort_is_allowed("provider/fable", "low"));
    assert!(runtime.model_reasoning_effort_is_allowed("provider/fable", "high"));
    assert!(!runtime.model_reasoning_effort_is_allowed("provider/fable", "max"));
    assert!(!runtime.model_reasoning_effort_is_allowed("provider/unknown", "low"));
    runtime
        .set_model_reasoning_allowed_levels(BTreeMap::from([(
            "provider/fable".to_string(),
            vec!["high".to_string()],
        )]))
        .unwrap();
    assert!(runtime.model_reasoning_effort_is_allowed("provider/fable", "high"));
    assert!(!runtime.model_reasoning_effort_is_allowed("provider/fable", "low"));
    assert_eq!(
        runtime.api_source_candidate_ids(),
        HashSet::from(["source-confirmed".to_string(), "source-unknown".to_string(),])
    );
}

#[tokio::test]
async fn non_claude_source_catalog_preserves_source_declared_efforts_and_uses_medium_auto_default()
{
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["grok-4.5", "glm-5.2"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();

    runtime.remember_source_model_manifest(
        "source-1",
        serde_json::json!({
            "data": [
                {
                    "id": "grok-4.5",
                    "reasoningEffortModes": [
                        "low", "medium", "high", "xhigh", "max", "very_high"
                    ],
                    "defaultReasoningLevel": "very_high",
                    "reasoningProbe": confirmed_reasoning_probe()
                },
                {
                    "id": "glm-5.2",
                    "reasoningEffortModes": ["low", "medium", "high", "xhigh", "max"],
                    "defaultReasoningLevel": "max",
                    "reasoningProbe": confirmed_reasoning_probe()
                }
            ]
        }),
        now_ms,
    );

    let metadata = runtime
        .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
        .await;

    for (model_id, expected) in [
        (
            "grok-4.5",
            serde_json::json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "low"},
                    {"effort": "medium", "description": "medium"},
                    {"effort": "high", "description": "high"},
                    {"effort": "xhigh", "description": "xhigh"},
                    {"effort": "max", "description": "max"},
                    {"effort": "very_high", "description": "very_high"}
                ],
                "default_reasoning_level": "medium"
            }),
        ),
        (
            "glm-5.2",
            serde_json::json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "low"},
                    {"effort": "medium", "description": "medium"},
                    {"effort": "high", "description": "high"},
                    {"effort": "xhigh", "description": "xhigh"},
                    {"effort": "max", "description": "max"}
                ],
                "default_reasoning_level": "medium"
            }),
        ),
    ] {
        assert_eq!(
            metadata.reasoning_catalog_templates[model_id],
            expected.as_object().unwrap().clone()
        );
    }
}

#[tokio::test]
async fn source_catalog_does_not_cross_model_reasoning_metadata() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["grok-4.5", "glm-5.2"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();

    runtime.remember_source_model_manifest(
        "source-1",
        serde_json::json!({
            "data": [
                {
                    "id": "grok-4.5",
                    "reasoningEffortModes": ["low", "very_high"],
                    "defaultReasoningLevel": "very_high",
                    "reasoningProbe": confirmed_reasoning_probe()
                },
                {"id": "glm-5.2"}
            ]
        }),
        now_ms,
    );

    let metadata = runtime
        .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
        .await;

    assert!(metadata
        .reasoning_catalog_templates
        .contains_key("grok-4.5"));
    assert!(!metadata.reasoning_catalog_templates.contains_key("glm-5.2"));
}

#[tokio::test]
async fn known_group_modes_reach_codex_without_being_reported_as_detected() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["vendor/claude-fable-5", "vendor/gpt-future"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();

    runtime.remember_source_model_manifest(
        "source-1",
        serde_json::json!({
            "data": [
                {"id": "vendor/claude-fable-5"},
                {"id": "vendor/gpt-future"}
            ]
        }),
        now_ms,
    );

    let metadata = runtime
        .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
        .await;

    assert!(!metadata
        .reasoning_catalog_templates
        .contains_key("vendor/claude-fable-5"));
    assert!(runtime
        .confirmed_source_reasoning_levels("vendor/claude-fable-5")
        .is_empty());
    assert!(!metadata
        .reasoning_catalog_templates
        .contains_key("vendor/gpt-future"));
    assert!(runtime
        .confirmed_source_reasoning_levels("vendor/gpt-future")
        .is_empty());
}

#[tokio::test]
async fn provider_reasoning_modes_override_known_model_fallback_for_routing() {
    let runtime = GatewayRuntime::from_pool(
        vec![RuntimeSource::unrestricted(source(
            "source-1",
            "upstream-secret",
            &["gpt-5.6-terra"],
        ))],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let authenticated = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    runtime.remember_source_model_manifest(
        "source-1",
        serde_json::json!({
            "data": [{
                "id": "gpt-5.6-terra",
                "reasoningEffortModes": ["ultra"],
                "reasoningProbe": confirmed_reasoning_probe()
            }]
        }),
        current_time_ms(),
    );

    runtime
        .codex_source_model_metadata(&authenticated, &[WireApi::Responses], current_time_ms())
        .await;

    assert_eq!(
        runtime.model_reasoning_allowed_levels("gpt-5.6-terra"),
        vec!["ultra".to_string()]
    );
    assert!(runtime.candidate_reasoning_effort_is_allowed("source-1", "gpt-5.6-terra", "ultra"));
    assert!(!runtime.model_reasoning_effort_is_allowed("gpt-5.6-terra", "max"));
}

#[tokio::test]
async fn codex_source_metadata_marks_bridge_images_but_requires_native_declaration() {
    let mut bridged = RuntimeSource::unrestricted(source(
        "source-bridge",
        "bridge-secret",
        &["vendor/claude-fable-5"],
    ));
    bridged.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::ResponsesToMessages,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["vendor/claude-fable-5".into()],
    }];
    let mut native = RuntimeSource::unrestricted(source(
        "source-native",
        "native-secret",
        &["provider/text-only"],
    ));
    native.protocol_bindings = vec![SourceProtocolBinding {
        wire_api: WireApi::Responses,
        adapter: SourceAdapter::Native,
        reasoning_mode: MessagesReasoningMode::Disabled,
        cache_write_ttl: Default::default(),
        model_ids: vec!["provider/text-only".into()],
    }];
    let runtime = GatewayRuntime::from_pool(
        vec![bridged, native],
        vec![RuntimeLocalKey::unrestricted(key("key-1", "local-secret"))],
        GatewayRuntimeOptions::default(),
        Arc::new(|_| {}),
    )
    .unwrap();
    let key = runtime
        .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
        .unwrap();
    let now_ms = current_time_ms();

    runtime.remember_source_model_manifest(
        "source-bridge",
        serde_json::json!({"data": [{"id": "vendor/claude-fable-5"}]}),
        now_ms,
    );
    runtime.remember_source_model_manifest(
        "source-native",
        serde_json::json!({
            "data": [{"id": "provider/text-only", "input_modalities": ["text"]}]
        }),
        now_ms,
    );

    let metadata = runtime
        .codex_source_model_metadata(&key, &[WireApi::Responses], now_ms)
        .await;

    assert!(metadata.image_models.contains("vendor/claude-fable-5"));
    assert!(!metadata.image_models.contains("provider/text-only"));
}

#[test]
fn messages_bridge_hides_provider_efforts_it_cannot_translate() {
    let configured_models = ["provider/fable"]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    let manifest = serde_json::json!({
        "data": [{
            "id": "provider/fable",
            "reasoningEffortModes": ["low", "provider-defined"],
            "supportsReasoningSummaryParameter": true,
            "supportsReasoningSummaries": true,
            "defaultReasoningSummary": "detailed",
            "reasoningProbe": confirmed_reasoning_probe()
        }]
    });
    let capabilities = source_reasoning_capabilities(&manifest, &configured_models)
        .remove("provider/fable")
        .unwrap();

    for reasoning_mode in [
        MessagesReasoningMode::Budget,
        MessagesReasoningMode::Adaptive,
    ] {
        let template = source_reasoning_for_route(
            capabilities.clone(),
            SourceAdapter::ResponsesToMessages,
            reasoning_mode,
        )
        .unwrap()
        .codex_catalog_template();

        assert_eq!(
            template,
            serde_json::json!({
                "supported_reasoning_levels": [
                    {"effort": "low", "description": "low"}
                ]
            })
            .as_object()
            .unwrap()
            .clone()
        );
    }
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
    assert_eq!(
        cheapest_image_main_model(&models).as_deref(),
        Some("gpt-5.4-mini")
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
