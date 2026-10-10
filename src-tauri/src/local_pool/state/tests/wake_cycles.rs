use super::*;

#[test]
fn weekly_reset_fingerprint_is_persisted_per_account_and_cycle() {
    let root = temp_root("weekly-reset-fingerprint");
    let state = DesktopState::open(root.clone()).unwrap();
    assert!(!state
        .weekly_reset_was_applied("account-1", "cycle-1")
        .unwrap());
    state
        .mark_weekly_reset_applied("account-1", "cycle-1")
        .unwrap();
    assert!(state
        .weekly_reset_was_applied("account-1", "cycle-1")
        .unwrap());
    assert!(!state
        .weekly_reset_was_applied("account-1", "cycle-2")
        .unwrap());
    drop(state);

    let reopened = DesktopState::open(root.clone()).unwrap();
    assert!(reopened
        .weekly_reset_was_applied("account-1", "cycle-1")
        .unwrap());
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn usage_callback_persists_before_returning() {
    let root = std::env::temp_dir().join(format!("zenith-relay-state-{}", uuid::Uuid::new_v4()));
    let state = DesktopState::open(root.clone()).unwrap();
    state
        .store()
        .unwrap()
        .upsert_source(ProviderSourceRecord {
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            base_url: "https://example.test/v1".into(),
            secret_ref: "source:source_1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_config: Default::default(),
            protocol_bindings: Vec::new(),
            models: vec!["gpt-test".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: Default::default(),
            detected_model_prices: Default::default(),
            last_used_at: None,
            last_test_at: None,
            last_test_status: None,
            last_error: None,
        })
        .unwrap();
    state
        .store()
        .unwrap()
        .upsert_key(LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "Default".into(),
            enabled: true,
            system: false,
            secret_ref: "key:key_1".into(),
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        })
        .unwrap();

    (state.usage_callback())(UsageEvent {
        request_id: "req_callback".into(),
        attempt: 1,
        local_key_id: "key_1".into(),
        source_id: "source_1".into(),
        candidate_id: Some("source_1".into()),
        account_id: None,
        account_token_generation: None,
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-test".into()),
        resolved_model: Some("gpt-test".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: zenith_relay_core::UsageTransport::Http,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success: true,
        http_status: 200,
        error_category: None,
        tool_use: zenith_relay_core::ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: Some(0),
        latency_ms: 7,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: Some(2),
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: Some(3),
        total_tokens: Some(5),
        upstream_error: None,
        quota_snapshot: None,
    });

    let logs = state.telemetry.list(10).unwrap();
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].request_id, "req_callback");
    assert_eq!(logs[0].total_tokens, Some(5));
    let store = state.store().unwrap();
    assert!(store.source("source_1").unwrap().last_used_at.is_some());
    assert!(store.key("key_1").unwrap().last_used_at.is_some());
    drop(store);
    drop(state);
    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert!(reopened.source("source_1").unwrap().last_used_at.is_some());
    assert!(reopened.key("key_1").unwrap().last_used_at.is_some());
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn persisted_passive_quota_defers_the_desktop_poll_without_touching_models() {
    let root = temp_root("passive-quota");
    let state = DesktopState::open(root.clone()).unwrap();
    let now = now_ms();
    let mut account = account_record("account-1");
    account.account.subscription.active_until_ms = Some(now + 3_600_000);
    account.account.subscription.updated_at_ms = Some(now);
    state
        .store()
        .unwrap()
        .replace_accounts_and_keys(vec![account.clone()], vec![key_record("key-1")])
        .unwrap();
    let identity = state
        .store()
        .unwrap()
        .account_refresh_scope("account-1")
        .unwrap()
        .1
        .identity();
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
                        refresh_value: Err(
                            crate::local_pool::error::LocalPoolError::invalid_state(
                                "synthetic read",
                            ),
                        ),
                        outcome: RefreshOutcome::Success,
                    }
                })
            },
        )
        .unwrap();
    let mut quota = account.account.quota;
    quota.primary.as_mut().unwrap().observed_at_ms = now;
    quota.primary.as_mut().unwrap().reset_at_ms = None;
    quota.updated_at_ms = Some(now);
    let mut event = account_usage_event("req-passive", true);
    event.quota_snapshot = Some(quota.clone());
    (state.usage_callback())(event);
    assert_eq!(
        state
            .store()
            .unwrap()
            .account("account-1")
            .unwrap()
            .account
            .quota,
        quota
    );
    assert!(matches!(
        state.refresh.freshness(&identity, RefreshKind::Quota),
        RefreshFreshness::Fresh { .. }
    ));
    assert_eq!(
        state.refresh.freshness(&identity, RefreshKind::Models),
        RefreshFreshness::Unknown
    );
    state.refresh.shutdown().await;
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn quota_classification_queues_refresh_without_blocking_the_account() {
    let mut account = account_record("account-quota");
    let mut event = account_status_event("account-quota", 403, Some("*"), Some(60_000), 1);
    event.error_category = Some("upstream_quota_exhausted".into());

    assert!(apply_account_usage_state(
        &mut account,
        &event,
        100,
        None,
        None,
        false,
    ));
    assert_eq!(account.account.health, AccountHealthState::Healthy);
    assert_eq!(account.account.last_error_code, None);
    assert_eq!(
        account
            .account
            .quota
            .primary
            .as_ref()
            .unwrap()
            .available_basis_points,
        Some(0)
    );
    assert_eq!(
        account
            .account
            .quota
            .primary
            .as_ref()
            .unwrap()
            .explicitly_full,
        Some(false)
    );
    assert!(!account.account.quota.limit_reached);
}
#[test]
fn account_usage_persists_natural_wake_completion_across_restart() {
    let root = temp_root("natural-use");
    let task = wake_task("task-1", WakeExecutionPolicy::Automatic);
    let state = DesktopState::open(root.clone()).unwrap();
    {
        let mut store = state.store().unwrap();
        let mut automations = store.automations().clone();
        automations.tasks = vec![task.clone()];
        store
            .replace_account_state(
                vec![account_record("account-1")],
                vec![key_record("key-1")],
                automations,
            )
            .unwrap();
    }
    let account = state
        .store()
        .unwrap()
        .account("account-1")
        .unwrap()
        .account
        .clone();
    assert!(matches!(
        state
            .evaluate_wake_transition(&task, &account, &wake_transition(), &wake_policy(), 110,)
            .unwrap(),
        WakeDecision::Scheduled(_)
    ));
    let permit = state
        .claim_due_automatic_wakes(110, 1)
        .unwrap()
        .pop()
        .unwrap();
    assert!(state.is_wake_permit_active(&permit).unwrap());

    (state.usage_callback())(account_usage_event("req-failed", false));
    {
        let store = state.store().unwrap();
        assert!(store
            .account("account-1")
            .unwrap()
            .account
            .last_used_at_ms
            .is_none());
        assert!(wake_coordinator(store.automations())
            .unwrap()
            .pending()
            .is_empty());
    }
    assert!(state.is_wake_permit_active(&permit).unwrap());
    (state.usage_callback())(account_usage_event("req-natural-use", true));
    assert!(!state.is_wake_permit_active(&permit).unwrap());
    assert!(!state
        .complete_wake(
            permit,
            WakeCompletion {
                outcome: zenith_relay_core::automations::WakeCompletionOutcome::Unconfirmed,
                completed_at_ms: now_ms(),
                latency_ms: Some(1),
                input_tokens: Some(1),
                output_tokens: Some(1),
                error_code: None,
            },
        )
        .unwrap());
    drop(state);

    let reopened = DesktopState::open(root.clone()).unwrap();
    let store = reopened.store().unwrap();
    assert!(store
        .account("account-1")
        .unwrap()
        .account
        .last_used_at_ms
        .is_some());
    assert!(store.key("key-1").unwrap().last_used_at.is_some());
    let coordinator = wake_coordinator(store.automations()).unwrap();
    assert!(coordinator.pending().is_empty());
    let history = coordinator.state().history().back().unwrap();
    assert_eq!(history.outcome, WakeOutcome::SkippedAlreadyStarted);
    assert!(history.model_id.is_none());
    assert!(history.input_tokens.is_none());
    assert!(history.output_tokens.is_none());
    drop(store);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn reopening_migrates_manual_rules_and_their_pending_cycles_to_automatic() {
    let root = temp_root("wake-automatic-migration");
    let task = wake_task("task-legacy", WakeExecutionPolicy::RequireConfirmation);
    let mut disabled = wake_task("task-disabled", WakeExecutionPolicy::RequireConfirmation);
    disabled.enabled = false;
    let state = DesktopState::open(root.clone()).unwrap();
    let account = account_record("account-1");
    {
        let mut store = state.store().unwrap();
        let mut automations = store.automations().clone();
        automations.tasks = vec![task.clone(), disabled];
        store
            .replace_account_state(vec![account.clone()], Vec::new(), automations)
            .unwrap();
    }
    assert!(matches!(
        state
            .evaluate_wake_transition(
                &task,
                &account.account,
                &wake_transition(),
                &wake_policy(),
                110,
            )
            .unwrap(),
        WakeDecision::Scheduled(_)
    ));
    let pending = state.wake_snapshot().unwrap().pending();
    assert!(state.claim_due_automatic_wakes(110, 1).unwrap().is_empty());
    drop(state);

    for _ in 0..2 {
        let reopened = DesktopState::open(root.clone()).unwrap();
        let store = reopened.store().unwrap();
        let mut automatic = task.clone();
        automatic.execution_policy = WakeExecutionPolicy::Automatic;
        assert_eq!(store.automations().tasks[0], automatic);
        assert!(!store.automations().tasks[1].enabled);
        assert_eq!(
            store.automations().tasks[1].execution_policy,
            WakeExecutionPolicy::Automatic
        );
        assert_eq!(reopened.wake_snapshot().unwrap().pending(), pending);
        assert_eq!(reopened.next_automatic_wake_due().unwrap(), Some(110));
        drop(store);
        drop(reopened);
    }
    let reopened = DesktopState::open(root.clone()).unwrap();
    let permits = reopened.claim_due_automatic_wakes(110, 2).unwrap();
    assert_eq!(permits.len(), 1);
    assert_eq!(permits[0].task_id, task.id);
    assert!(!permits[0].requires_confirmation);
    assert!(reopened
        .claim_due_automatic_wakes(110, 2)
        .unwrap()
        .is_empty());
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn automatic_wake_claim_does_not_claim_confirmation_cycle() {
    let root = temp_root("wake-confirmation");
    let automatic = wake_task("task-auto", WakeExecutionPolicy::Automatic);
    let confirmation = wake_task("task-confirm", WakeExecutionPolicy::RequireConfirmation);
    let state = DesktopState::open(root.clone()).unwrap();
    let account = account_record("account-1");
    {
        let mut store = state.store().unwrap();
        let mut automations = store.automations().clone();
        automations.tasks = vec![automatic.clone(), confirmation.clone()];
        store
            .replace_account_state(vec![account.clone()], Vec::new(), automations)
            .unwrap();
    }
    assert!(matches!(
        state
            .evaluate_wake_transition(
                &automatic,
                &account.account,
                &wake_transition(),
                &wake_policy(),
                110,
            )
            .unwrap(),
        WakeDecision::Scheduled(_)
    ));
    assert!(matches!(
        state
            .evaluate_wake_transition(
                &confirmation,
                &account.account,
                &wake_transition_with_fingerprint("cycle-2"),
                &wake_policy(),
                110,
            )
            .unwrap(),
        WakeDecision::Scheduled(_)
    ));

    let mut permits = state.claim_due_automatic_wakes(110, 8).unwrap();
    assert_eq!(permits.len(), 1);
    assert_eq!(permits[0].task_id, automatic.id);
    assert_eq!(state.next_automatic_wake_due().unwrap(), None);
    let mut confirmation_permits = state.claim_due_confirmation_wakes(110, 8).unwrap();
    assert_eq!(confirmation_permits.len(), 1);
    assert_eq!(confirmation_permits[0].task_id, confirmation.id);
    assert!(state
        .complete_wake(
            permits.remove(0),
            WakeCompletion {
                outcome: zenith_relay_core::automations::WakeCompletionOutcome::Confirmed,
                completed_at_ms: 120,
                latency_ms: Some(10),
                input_tokens: Some(1),
                output_tokens: Some(1),
                error_code: None,
            },
        )
        .unwrap());
    assert!(state
        .complete_wake(
            confirmation_permits.remove(0),
            WakeCompletion {
                outcome: zenith_relay_core::automations::WakeCompletionOutcome::Confirmed,
                completed_at_ms: 120,
                latency_ms: Some(10),
                input_tokens: Some(1),
                output_tokens: Some(1),
                error_code: None,
            },
        )
        .unwrap());
    assert_eq!(
        state
            .remove_pending_wakes_for_account("missing-account")
            .unwrap(),
        0
    );
    assert_eq!(
        state
            .remove_pending_wakes_for_task(&confirmation.id)
            .unwrap(),
        0
    );
    assert!(state.next_automatic_wake_due().unwrap().is_none());
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}
