use super::account_runtime::{account_summary, runtime_account, AccountSummaryInputs};
use super::*;
use crate::test_fixtures::pooled_source;
use crate::{
    config::Config,
    store::{Store, Vault},
};
use std::collections::BTreeMap;
use tempfile::TempDir;
use zenith_relay_core::accounts::AccountAuthState;
use zenith_relay_core::quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription};
use zenith_relay_core::{
    protocol::{OperationalStatus, ProxyMode, UsageQuery},
    ApiEquivalentSummary, ApiModelPriceOverride, CandidateQuota, DefaultServiceTier, UsageEvent,
    WireApi,
};

fn snapshot_test_state(root: &TempDir) -> Arc<AppState> {
    let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
    let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
    AppState::new(config, store, vault).unwrap()
}

fn snapshot_test_source(id: &str, model: &str) -> SourceRecord {
    let mut source = pooled_source(id, model);
    source.name = "Snapshot source".into();
    source
}

fn snapshot_test_account(id: &str, model: &str) -> ServerAccountRecord {
    ServerAccountRecord {
        id: id.into(),
        label: "Snapshot account".into(),
        identity_hint: "snapshot-account".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        source_id: "openai_codex".into(),
        secret_ref: format!("account:{id}"),
        provider_family: Some("openai".into()),
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        models: vec![model.into()],
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
    }
}

#[test]
fn snapshot_preserves_persisted_model_policy_and_missing_secret_warning() {
    let root = TempDir::new().unwrap();
    let state = snapshot_test_state(&root);
    let source = snapshot_test_source("source-snapshot", "gpt-5.4");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    state
        .store
        .save_account(&snapshot_test_account(
            "account-missing",
            "gpt-account-test",
        ))
        .unwrap();
    state
        .store
        .set_hidden_models(vec!["gpt-5.4".into()])
        .unwrap();
    state
        .store
        .set_model_price_overrides(BTreeMap::from([(
            "gpt-5.4".into(),
            ApiModelPriceOverride {
                input_micro_usd_per_million: 1_000,
                cached_input_micro_usd_per_million: Some(100),
                cache_write_5m_micro_usd_per_million: Some(1_500),
                cache_write_1h_micro_usd_per_million: Some(2_000),
                output_micro_usd_per_million: 3_000,
            },
        )]))
        .unwrap();

    let snapshot = state.snapshot().unwrap();

    assert_eq!(snapshot.runtime_target.kind, "remote");
    assert!(!snapshot.gateway.running);
    assert_eq!(snapshot.gateway.candidate_count, 1);
    assert!(snapshot.gateway.visible_model_ids.is_empty());
    assert_eq!(snapshot.sources.len(), 1);
    assert!(snapshot.sources[0].secret_available);
    assert_eq!(snapshot.accounts.len(), 1);
    assert!(!snapshot.accounts[0].secret_available);
    assert_eq!(
        snapshot.warnings,
        vec!["account_secret_missing:account-missing"]
    );

    let model = snapshot.gateway.models.first().unwrap();
    assert_eq!(model.id, "gpt-5.4");
    assert!(!model.enabled);
    assert!(model.custom_price);
    assert_eq!(model.input_micro_usd_per_million, Some(1_000));
    assert_eq!(model.cached_input_micro_usd_per_million, Some(100));
    // Cache creation prices are meaningful only for a confirmed native
    // Messages route, never for this Responses source.
    assert_eq!(model.cache_write_5m_micro_usd_per_million, None);
    assert_eq!(model.cache_write_1h_micro_usd_per_million, None);
    assert_eq!(model.output_micro_usd_per_million, Some(3_000));
}

#[tokio::test]
async fn snapshot_reports_the_active_runtime_candidate_order() {
    let root = TempDir::new().unwrap();
    let state = snapshot_test_state(&root);
    let source = snapshot_test_source("source-runtime", "gpt-runtime-test");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    state.rebuild_runtime().await.unwrap();

    let snapshot = state.snapshot().unwrap();

    assert!(snapshot.gateway.running);
    assert_eq!(snapshot.gateway.candidate_count, 1);
    assert_eq!(snapshot.gateway.visible_model_ids, ["gpt-runtime-test"]);
    assert_eq!(snapshot.gateway.routing_order.len(), 4);
    assert_eq!(snapshot.gateway.routing_order[0].candidate_id, source.id);
    assert!(snapshot.gateway.routing_order.iter().all(|candidate| {
        candidate.available
            && (candidate.candidate_id == source.id
                || candidate
                    .candidate_id
                    .strip_prefix(&source.id)
                    .is_some_and(|suffix| suffix.starts_with("::")))
    }));
    assert_eq!(
        snapshot.sources[0].operational_status,
        OperationalStatus::Rotation
    );

    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn publishing_replacement_retires_the_previous_server_runtime() {
    let root = TempDir::new().unwrap();
    let state = snapshot_test_state(&root);
    let source = snapshot_test_source("replacement", "model-replacement");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let previous = state.runtime().unwrap().unwrap();
    let order = || previous.candidate_runtime_order_for_key(crate::state::SYSTEM_GATEWAY_KEY_ID);
    assert!(order().iter().any(|candidate| candidate.available));

    state.rebuild_runtime().await.unwrap();
    let current = state.runtime().unwrap().unwrap();
    assert!(!Arc::ptr_eq(&previous, &current));
    assert!(order().iter().all(|candidate| !candidate.available));
    assert!(current
        .candidate_runtime_order_for_key(crate::state::SYSTEM_GATEWAY_KEY_ID)
        .iter()
        .any(|candidate| candidate.available));
    state.shutdown_runtime().await.unwrap();
    assert!(current
        .candidate_runtime_order_for_key(crate::state::SYSTEM_GATEWAY_KEY_ID)
        .iter()
        .all(|candidate| !candidate.available));
}

#[tokio::test]
async fn failed_rollback_retires_the_old_runtime_instead_of_serving_stale_permissions() {
    let root = TempDir::new().unwrap();
    let state = snapshot_test_state(&root);
    let source = snapshot_test_source("previous", "model-previous");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let previous = state.runtime().unwrap().unwrap();

    let invalid = snapshot_test_account("invalid-build", "model-invalid");
    state.store.save_account(&invalid).unwrap();
    state
        .vault
        .save(&invalid.secret_ref, "not-a-credential")
        .unwrap();
    let error = state
        .rebuild_runtime_or_rollback(|| Err("synthetic rollback unavailable".into()))
        .await
        .unwrap_err();
    assert!(error.contains("failed to restore persisted state"));
    assert!(state.runtime().unwrap().is_none());
    assert!(previous
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
}

#[tokio::test]
async fn membership_refresh_applies_saved_routing_without_losing_runtime_state() {
    use zenith_relay_core::{
        PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy,
    };

    let root = TempDir::new().unwrap();
    let state = snapshot_test_state(&root);
    let mut primary = snapshot_test_source("primary", "gpt-test");
    primary.in_pool = false;
    let fallback = snapshot_test_source("fallback", "gpt-test");
    for source in [&primary, &fallback] {
        state.store.save_source(source).unwrap();
        state
            .vault
            .save(&source.secret_ref, "synthetic-source-key")
            .unwrap();
    }
    let mut routing = state.store.routing_policy().unwrap();
    let policy = PoolRoutingPolicy {
        mode: PoolRoutingMode::InOrder,
        members: [(&primary, 1), (&fallback, 0)]
            .into_iter()
            .map(|(source, max_concurrency)| PoolRoutingMember {
                kind: PoolMemberKind::Source,
                id: source.id.clone(),
                weight: 3,
                max_concurrency,
            })
            .collect(),
        ..Default::default()
    };
    routing.pool_routing = Some(policy.clone());
    state.store.set_routing_policy(&routing).unwrap();
    state.rebuild_runtime().await.unwrap();
    let runtime = state.runtime().unwrap().unwrap();
    let next = || {
        runtime
            .candidate_runtime_order_for_key(crate::state::SYSTEM_GATEWAY_KEY_ID)
            .into_iter()
            .find(|candidate| candidate.next_for_new_request)
            .map(|candidate| candidate.candidate_id)
    };
    assert_eq!(next().as_deref(), Some("fallback"));

    let retry_at = now_ms() + 60_000;
    runtime.set_candidate_cooldown(&fallback.id, "gpt-test", retry_at);
    primary.in_pool = true;
    state.store.save_source(&primary).unwrap();
    assert!(state.refresh_internal_gateway_key_scopes(&runtime).unwrap());

    assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));
    assert_eq!(next().as_deref(), Some("primary"));
    let snapshot = state.snapshot().unwrap();
    assert_eq!(snapshot.gateway.pool_routing, Some(policy));
    assert_eq!(
        snapshot
            .gateway
            .routing_order
            .iter()
            .find(|candidate| candidate.candidate_id == fallback.id)
            .unwrap()
            .next_retry_at_ms,
        Some(retry_at)
    );
    primary.in_pool = false;
    state.store.save_source(&primary).unwrap();
    assert!(state.refresh_internal_gateway_key_scopes(&runtime).unwrap());
    assert!(next().is_none());
    runtime.clear_candidate_cooldown(&fallback.id, "gpt-test");
    assert_eq!(next().as_deref(), Some("fallback"));
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn account_snapshot_tracks_cooldown_recovery_and_missing_candidate() {
    let root = TempDir::new().unwrap();
    let state = snapshot_test_state(&root);
    let account = snapshot_test_account("snapshot-account", "gpt-test");
    let credential = AccountCredential {
        access_token: "synthetic-access".into(),
        refresh_token: None,
        id_token: None,
        expires_at_ms: None,
        issued_at_ms: 1,
        generation: 0,
        chatgpt_account_id: "synthetic-provider".into(),
        responses_url: "http://127.0.0.1:9/v1/responses".into(),
        proxy_url: None,
        agent_private_key: None,
        agent_runtime_id: None,
        agent_task_id: None,
    };
    state.store.save_account(&account).unwrap();
    state
        .vault
        .save(
            &account.secret_ref,
            &serde_json::to_string(&credential).unwrap(),
        )
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let runtime = state.runtime().unwrap().unwrap();
    assert_eq!(
        state.snapshot().unwrap().accounts[0].operational_status,
        OperationalStatus::Rotation
    );
    assert!(runtime.set_candidate_cooldown(&account.id, "gpt-test", now_ms() + 60_000));
    assert_eq!(
        state.snapshot().unwrap().accounts[0].operational_status,
        OperationalStatus::Unavailable
    );
    assert!(runtime.clear_candidate_cooldown(&account.id, "gpt-test"));
    assert_eq!(
        state.snapshot().unwrap().accounts[0].operational_status,
        OperationalStatus::Rotation
    );
    assert!(runtime.remove_candidate(&account.id));
    assert_eq!(
        state.snapshot().unwrap().accounts[0].operational_status,
        OperationalStatus::Unavailable
    );
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn rebuild_runtime_accepts_messages_sources_for_the_multi_protocol_system_key() {
    let root = TempDir::new().unwrap();
    let state = snapshot_test_state(&root);
    let mut messages = snapshot_test_source("source-messages", "claude-native-test");
    messages.wire_api = WireApi::Messages;
    state.store.save_source(&messages).unwrap();
    state
        .vault
        .save(&messages.secret_ref, "synthetic-source-key")
        .unwrap();

    state.rebuild_runtime().await.unwrap();
    let snapshot = state.snapshot().unwrap();
    assert!(snapshot.gateway.running);
    assert_eq!(snapshot.gateway.candidate_count, 1);
    assert_eq!(snapshot.gateway.visible_model_ids, ["claude-native-test"]);

    let responses = snapshot_test_source("source-responses", "gpt-runtime-test");
    state.store.save_source(&responses).unwrap();
    state
        .vault
        .save(&responses.secret_ref, "synthetic-source-key")
        .unwrap();

    state.rebuild_runtime().await.unwrap();
    let snapshot = state.snapshot().unwrap();
    assert!(snapshot.gateway.running);
    assert_eq!(snapshot.gateway.candidate_count, 2);
    assert_eq!(
        snapshot.gateway.visible_model_ids,
        ["gpt-runtime-test", "claude-native-test"]
    );

    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn usage_writer_is_reused_and_flushes_before_shutdown() {
    let root = TempDir::new().unwrap();
    let config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
    let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
    let state = AppState::new(config, store.clone(), vault).unwrap();
    let first = state.usage_callback().unwrap();
    let second = state.usage_callback().unwrap();
    assert!(Arc::ptr_eq(&first, &second));

    first(UsageEvent {
        request_id: "req_shutdown_flush".into(),
        attempt: 1,
        local_key_id: "key_test".into(),
        source_id: "source_test".into(),
        candidate_id: Some("source_test".into()),
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
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: zenith_relay_core::ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: Some(0),
        latency_ms: 10,
        ttft_ms: Some(3),
        generation_ms: Some(7),
        input_tokens: Some(2),
        cached_input_tokens: Some(1),
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: Some(3),
        total_tokens: Some(5),
        upstream_error: None,
        quota_snapshot: None,
    });
    state.shutdown_runtime().await.unwrap();

    let usage = store.usage_page(&UsageQuery::default()).unwrap();
    assert_eq!(usage.total, 1);
    assert_eq!(usage.events[0].request_id, "req_shutdown_flush");
}

#[test]
fn scheduler_uses_the_tightest_fresh_quota_window() {
    let window = |kind, available_basis_points| QuotaWindow {
        kind,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(available_basis_points),
        explicitly_full: None,
        reset_at_ms: None,
        window_minutes: None,
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: None,
        observed_at_ms: 1_000,
    };
    let quota = QuotaSnapshot {
        primary: Some(window(QuotaWindowKind::Primary, 9_000)),
        secondary: Some(window(QuotaWindowKind::Secondary, 2_500)),
        updated_at_ms: Some(1_000),
        ..Default::default()
    };
    assert_eq!(
        CandidateQuota::from_snapshot(&quota, 2_000, zenith_relay_core::QUOTA_STALE_AFTER_MS,),
        CandidateQuota::Available(2_500)
    );
    assert_eq!(
        CandidateQuota::from_snapshot(
            &quota,
            zenith_relay_core::QUOTA_STALE_AFTER_MS + 1_001,
            zenith_relay_core::QUOTA_STALE_AFTER_MS,
        ),
        CandidateQuota::Stale
    );
}

#[test]
fn free_accounts_route_like_other_pool_accounts() {
    let record = ServerAccountRecord {
        id: "account-free".into(),
        label: "Free".into(),
        identity_hint: "free-account".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        source_id: "codex".into(),
        secret_ref: "account:free".into(),
        provider_family: Some("openai".into()),
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        models: vec!["gpt-test".into()],
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        subscription: Subscription {
            plan_type: Some("free".into()),
            ..Subscription::default()
        },
        quota: QuotaSnapshot::default(),
        purchase_cost_micro_usd: None,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        created_at_ms: 1,
        last_used_at_ms: None,
        last_error_code: None,
        proxy_id: None,
        bypass_common_proxy: false,
    };
    let credential = AccountCredential {
        access_token: "access".into(),
        refresh_token: None,
        id_token: None,
        expires_at_ms: None,
        issued_at_ms: 1,
        generation: 0,
        chatgpt_account_id: "provider-account".into(),
        responses_url: "https://example.test/responses".into(),
        proxy_url: None,
        agent_private_key: None,
        agent_runtime_id: None,
        agent_task_id: None,
    };

    assert!(
        runtime_account(
            record.clone(),
            &credential,
            None,
            false,
            zenith_relay_core::QUOTA_STALE_AFTER_MS,
        )
        .enabled
    );
    let mut exhausted = record.clone();
    exhausted.quota = QuotaSnapshot {
        primary: Some(QuotaWindow {
            kind: QuotaWindowKind::Primary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(0),
            explicitly_full: None,
            reset_at_ms: Some(60_000),
            window_minutes: None,
            observed_at_ms: 1_000,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }),
        updated_at_ms: Some(1_000),
        ..Default::default()
    };
    assert!(
        runtime_account(
            exhausted,
            &credential,
            None,
            false,
            zenith_relay_core::QUOTA_STALE_AFTER_MS,
        )
        .enabled,
        "a quota wait must stay instantiated so a refresh can re-enable it"
    );
    let summary = account_summary(
        &record,
        AccountSummaryInputs {
            secret_available: true,
            basis_points_available: true,
            basis_points_enabled: false,
            proxy_mode: ProxyMode::Direct,
            proxy_available: true,
            api_equivalent: ApiEquivalentSummary::default(),
            quota_window_usage: None,
            quota_stale_after_ms: zenith_relay_core::QUOTA_STALE_AFTER_MS,
        },
    );
    assert_eq!(summary.operational_status, OperationalStatus::Rotation);
    assert!(summary.enabled);
    assert!(summary.in_pool);
}
