use super::*;
use crate::{
    state::{AccountCredential, ServerAccountRecord},
    test_fixtures::test_app_state,
};
use std::sync::atomic::Ordering;
use tempfile::TempDir;
use zenith_relay_core::{
    quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind},
    scheduler::refresh::{
        service::RefreshRegistration, service::RefreshResult, RefreshFreshness, RefreshOutcome,
    },
    DefaultServiceTier, ToolUseDiagnostics, UsageEvent, WireApi,
};

fn test_account(id: &str) -> ServerAccountRecord {
    crate::test_fixtures::synthetic_server_account(id)
}

fn usage_event(request_id: &str, account_id: &str) -> UsageEvent {
    UsageEvent {
        request_id: request_id.to_string(),
        attempt: 1,
        local_key_id: "key_test".to_string(),
        source_id: "openai_codex".to_string(),
        candidate_id: Some(account_id.to_string()),
        account_id: Some(account_id.to_string()),
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-test".to_string()),
        resolved_model: Some("gpt-test".to_string()),
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
        latency_ms: 1,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: Some(1),
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: Some(1),
        total_tokens: Some(2),
        upstream_error: None,
        quota_snapshot: None,
    }
}

#[test]
fn missing_account_does_not_block_other_usage_updates_or_count_as_a_write_failure() {
    let root = TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let store = Arc::clone(&state.store);
    let vault = Arc::clone(&state.vault);
    for id in ["account_a", "account_b"] {
        let account = test_account(id);
        store.save_account(&account).unwrap();
        vault
            .save(
                &account.secret_ref,
                &serde_json::to_string(&AccountCredential {
                    oauth_client_kind: Default::default(),
                    chatgpt_user_id: None,
                    basis_points_headers: None,
                    access_token: "test-token".to_string(),
                    refresh_token: None,
                    id_token: None,
                    expires_at_ms: None,
                    issued_at_ms: 0,
                    generation: 0,
                    chatgpt_account_id: id.to_string(),
                    responses_url: "https://example.test/v1/responses".to_string(),
                    proxy_url: None,
                    agent_private_key: None,
                    agent_runtime_id: None,
                    agent_task_id: None,
                })
                .unwrap(),
            )
            .unwrap();
    }
    let batch = [
        QueuedUsage {
            event: usage_event("request_a", "account_a"),
            observed_at_ms: 10,
        },
        QueuedUsage {
            event: usage_event("request_missing", "account_missing"),
            observed_at_ms: 20,
        },
        QueuedUsage {
            event: usage_event("request_b", "account_b"),
            observed_at_ms: 30,
        },
    ];
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    persist_usage_batch(&state, &batch, runtime.handle());

    assert_eq!(
        store.account("account_a").unwrap().unwrap().last_used_at_ms,
        Some(10)
    );
    assert_eq!(
        store.account("account_b").unwrap().unwrap().last_used_at_ms,
        Some(30)
    );
    assert_eq!(state.failed_usage_writes.load(Ordering::Relaxed), 0);
}

#[test]
fn persisted_passive_quota_is_fresh_for_the_registered_account_only() {
    let root = TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let store = Arc::clone(&state.store);
    let now = now_ms();
    let mut account = test_account("account_a");
    account.subscription.active_until_ms = Some(now + 3_600_000);
    account.subscription.updated_at_ms = Some(now);
    store.save_account(&account).unwrap();
    let (_, fence) = store.account_refresh_scope(&account.id).unwrap();
    let identity = fence.identity();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let enter = runtime.enter();
    state
        .refresh
        .register(
            RefreshRegistration {
                identity: identity.clone(),
                kind: RefreshKind::Quota,
                origin: "https://provider.example.test".into(),
                active: true,
                automatic: true,
                due_now: false,
            },
            |_| {
                Box::pin(async {
                    RefreshResult {
                        refresh_value: Err("synthetic read".into()),
                        outcome: RefreshOutcome::Success,
                    }
                })
            },
        )
        .unwrap();
    drop(enter);
    let mut event = usage_event("request_passive", &account.id);
    event.quota_snapshot = Some(QuotaSnapshot {
        primary: Some(QuotaWindow {
            kind: QuotaWindowKind::Primary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(2_000),
            explicitly_full: None,
            reset_at_ms: None,
            window_minutes: None,
            observed_at_ms: now,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }),
        updated_at_ms: Some(now),
        ..QuotaSnapshot::default()
    });
    let batch = [QueuedUsage {
        event,
        observed_at_ms: now,
    }];
    persist_usage_batch(&state, &batch, runtime.handle());
    assert_eq!(
        store.account(&account.id).unwrap().unwrap().quota,
        batch[0].event.quota_snapshot.clone().unwrap()
    );
    assert!(matches!(
        state.refresh.freshness(&identity, RefreshKind::Quota),
        RefreshFreshness::Fresh { .. }
    ));
    assert_eq!(
        state.refresh.freshness(&identity, RefreshKind::Models),
        RefreshFreshness::Unknown
    );
    runtime.block_on(state.refresh.shutdown());
}
