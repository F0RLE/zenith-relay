use super::*;

#[test]
fn source_revision_is_optional_in_old_snapshots_and_non_secret_in_new_ones() {
    let mut source = source_summary("synthetic", &["test"]);
    let mut legacy = serde_json::to_value(&source).unwrap();
    legacy.as_object_mut().unwrap().remove("refreshState");
    assert!(legacy.get("refreshRevision").is_none());
    assert_eq!(
        serde_json::from_value::<SourceSummary>(legacy)
            .unwrap()
            .refresh_state,
        SourceRefreshState::default()
    );
    source.refresh_revision = Some(42);
    source.refresh_state.models = RefreshStatus::Stale;
    source.refresh_state.balance = RefreshStatus::Unsupported;
    let projection = serde_json::to_value(&source).unwrap();
    assert_eq!(
        projection
            .get("refreshRevision")
            .and_then(|value| value.as_u64()),
        Some(42)
    );
    assert_eq!(projection["refreshState"]["models"], "stale");
    assert_eq!(projection["refreshState"]["balance"], "unsupported");
    let mut old_account = serde_json::to_value(account_summary(true, &["test"])).unwrap();
    old_account.as_object_mut().unwrap().remove("refreshState");
    assert_eq!(
        serde_json::from_value::<AccountSummary>(old_account)
            .unwrap()
            .refresh_state,
        AccountRefreshState::default()
    );
    assert!(!projection.to_string().contains("credential"));
}
#[test]
fn usage_query_pagination_uses_bounded_defaults() {
    let mut query = UsageQuery {
        page: 0,
        page_size: 0,
        bucket_ms: Some(59_999),
        ..Default::default()
    };
    query.normalize_pagination();
    assert_eq!(query.page, 1);
    assert_eq!(query.page_size, 50);
    assert_eq!(query.bucket_ms, None);

    query.page = 9;
    query.page_size = 999;
    query.bucket_ms = Some(60_000);
    query.normalize_pagination();
    assert_eq!(query.page, 9);
    assert_eq!(query.page_size, 200);
    assert_eq!(query.bucket_ms, Some(60_000));
}
#[test]
fn runtime_and_native_account_helpers_keep_summary_rules_shared() {
    let runtime = [
        runtime_candidate("source::messages", CandidateKind::ApiSource, true),
        runtime_candidate("source::responses", CandidateKind::ApiSource, false),
        runtime_candidate("source", CandidateKind::OAuthAccount, true),
    ];
    assert!(source_runtime_available(&runtime, "source"));
    assert!(pooled_source_runtime_available(&runtime, "source"));
    let responses = [runtime_candidate(
        "source::responses_to_messages",
        CandidateKind::ApiSource,
        true,
    )];
    assert!(pooled_source_runtime_available(&responses, "source"));
    let legacy = [runtime_candidate("source", CandidateKind::ApiSource, true)];
    assert!(pooled_source_runtime_available(&legacy, "source"));
    assert!(!source_runtime_available(&runtime, "missing"));
    assert!(!source_runtime_available(&runtime, "sour"));

    let accounts = [
        account_summary(true, &["GPT-5"]),
        account_summary(false, &["other"]),
    ];
    assert!(model_has_native_account_route(&accounts, "gpt-5"));
    assert!(!model_has_native_account_route(&accounts, "other"));
}
#[test]
fn generated_ids_require_the_expected_prefix_and_hex_suffix() {
    assert!(valid_generated_id(
        "batch_0123456789abcdef0123456789ABCDEF",
        "batch_"
    ));
    assert!(!valid_generated_id("batch_0123456789abcdef", "batch_"));
    assert!(!valid_generated_id(
        "batch_0123456789abcdef0123456789abcdeg",
        "batch_"
    ));
    assert!(!valid_generated_id(
        "import_0123456789abcdef0123456789abcdef",
        "batch_"
    ));
}
#[test]
fn usage_summary_accepts_servers_without_reasoning_telemetry() {
    let summary: UsageSummary = serde_json::from_str(
            r#"{"id":1,"requestId":"req","localKeyId":"key","candidateKind":"source","candidateHint":"abc","requestedModel":null,"resolvedModel":null,"wireApi":"responses","success":true,"httpStatus":200,"errorCategory":null,"latencyMs":1,"inputTokens":2,"cachedInputTokens":null,"outputTokens":3,"totalTokens":5,"createdAtMs":1}"#,
        )
        .unwrap();

    assert_eq!(summary.tokens.reasoning_tokens, None);
    assert_eq!(summary.ttft_ms, None);
    assert!(!serde_json::to_value(&summary)
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("localKeyId"));

    let mut legacy_totals = serde_json::to_value(UsageTotals::default()).unwrap();
    let fields = legacy_totals.as_object_mut().unwrap();
    fields.remove("cacheWriteInputTokens");
    fields.remove("cacheWriteInputSamples");
    let totals: UsageTotals = serde_json::from_value(legacy_totals).unwrap();
    assert_eq!(totals.cache_write_input_tokens, 0);
    assert_eq!(totals.cache_write_input_samples, 0);
}
#[test]
fn operational_status_has_one_backend_precedence() {
    assert_eq!(
        operational_status(false, false, true, Some(true)),
        OperationalStatus::Disabled
    );
    assert_eq!(
        operational_status(true, true, false, Some(true)),
        OperationalStatus::Unavailable
    );
    assert_eq!(
        operational_status(true, true, true, Some(false)),
        OperationalStatus::QuotaWait
    );
    assert_eq!(
        operational_status(true, false, true, Some(false)),
        OperationalStatus::Unavailable
    );
    assert_eq!(
        operational_status(true, false, true, None),
        OperationalStatus::Rotation
    );
}
#[test]
fn account_operational_state_is_shared_and_does_not_invent_fresh_exhaustion() {
    let subscription = Subscription {
        plan_type: Some("plus".into()),
        active_until_ms: None,
        status: SubscriptionStatus::Active,
        updated_at_ms: Some(1),
    };
    let mut quota = QuotaSnapshot {
        primary: Some(QuotaWindow {
            kind: QuotaWindowKind::Primary,
            provider_cycle_id: None,
            window_start_ms: None,
            available_basis_points: Some(0),
            explicitly_full: None,
            reset_at_ms: None,
            window_minutes: None,
            observed_at_ms: 1,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }),
        updated_at_ms: Some(1),
        ..Default::default()
    };
    let state = account_operational_state(AccountOperationalInput {
        enabled: true,
        in_pool: true,
        draining: false,
        secret_available: true,
        proxy_available: true,
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        subscription: &subscription,
        quota: &quota,
        last_error_code: None,
        now_ms: 1_000,
        quota_stale_after_ms: 10,
    });
    assert_eq!(state.quota, CandidateQuota::Stale);
    assert_eq!(state.status, OperationalStatus::Rotation);
    assert!(state.routing_eligible);
    assert_eq!(state.routing_block_reason, None);

    quota.updated_at_ms = Some(1_000);
    quota.primary.as_mut().unwrap().available_basis_points = Some(5_000);
    let state = account_operational_state(AccountOperationalInput {
        enabled: true,
        in_pool: false,
        draining: false,
        secret_available: true,
        proxy_available: true,
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        subscription: &subscription,
        quota: &quota,
        last_error_code: None,
        now_ms: 1_000,
        quota_stale_after_ms: 10,
    });
    assert_eq!(state.status, OperationalStatus::Rotation);
    assert!(!state.routing_eligible);
    assert_eq!(
        state.routing_block_reason,
        Some(AccountRoutingBlockReason::NotInPool)
    );

    quota.primary.as_mut().unwrap().available_basis_points = Some(0);
    let state = account_operational_state(AccountOperationalInput {
        enabled: true,
        in_pool: true,
        draining: false,
        secret_available: true,
        proxy_available: true,
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        subscription: &subscription,
        quota: &quota,
        last_error_code: None,
        now_ms: 1_000,
        quota_stale_after_ms: 10,
    });
    assert_eq!(state.quota, CandidateQuota::Exhausted);
    assert_eq!(state.status, OperationalStatus::QuotaWait);
    assert_eq!(
        state.routing_block_reason,
        Some(AccountRoutingBlockReason::QuotaExhausted)
    );
    assert!(account_candidate_enabled(true, state.routing_block_reason));
}
#[test]
fn unavailable_credentials_always_win_over_pending_quota() {
    let subscription = Subscription {
        plan_type: Some("plus".into()),
        active_until_ms: None,
        status: SubscriptionStatus::Active,
        updated_at_ms: Some(1),
    };
    let state = account_operational_state(AccountOperationalInput {
        enabled: true,
        in_pool: true,
        draining: false,
        secret_available: false,
        proxy_available: true,
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        subscription: &subscription,
        quota: &QuotaSnapshot::default(),
        last_error_code: None,
        now_ms: 1_000,
        quota_stale_after_ms: 10,
    });
    assert_eq!(state.status, OperationalStatus::Unavailable);
    assert_eq!(
        state.routing_block_reason,
        Some(AccountRoutingBlockReason::SecretUnavailable)
    );
}
#[test]
fn expired_chatgpt_entitlement_does_not_block_working_codex_account() {
    let subscription = Subscription {
        plan_type: Some("business".into()),
        active_until_ms: Some(900),
        status: SubscriptionStatus::Expired,
        updated_at_ms: Some(900),
    };
    let state = account_operational_state(AccountOperationalInput {
        enabled: true,
        in_pool: true,
        draining: false,
        secret_available: true,
        proxy_available: true,
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        subscription: &subscription,
        quota: &QuotaSnapshot {
            primary: Some(QuotaWindow {
                kind: QuotaWindowKind::Primary,
                provider_cycle_id: None,
                window_start_ms: None,
                available_basis_points: Some(8_000),
                explicitly_full: None,
                reset_at_ms: None,
                window_minutes: None,
                observed_at_ms: 1_000,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
            }),
            updated_at_ms: Some(1_000),
            ..Default::default()
        },
        last_error_code: None,
        now_ms: 1_000,
        quota_stale_after_ms: 10_000,
    });

    assert_eq!(state.health, CandidateHealth::Healthy);
    assert!(state.routing_eligible);
    assert_eq!(state.routing_block_reason, None);
}
#[test]
fn forbidden_chatgpt_subscription_still_blocks_routing() {
    let subscription = Subscription {
        plan_type: Some("business".into()),
        status: SubscriptionStatus::Forbidden,
        ..Default::default()
    };
    let state = account_operational_state(AccountOperationalInput {
        enabled: true,
        in_pool: true,
        draining: false,
        secret_available: true,
        proxy_available: true,
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        subscription: &subscription,
        quota: &QuotaSnapshot::default(),
        last_error_code: None,
        now_ms: 1_000,
        quota_stale_after_ms: 10_000,
    });

    assert_eq!(state.health, CandidateHealth::Blocked);
    assert!(!state.routing_eligible);
    assert_eq!(
        state.routing_block_reason,
        Some(AccountRoutingBlockReason::SubscriptionForbidden)
    );
}
#[test]
fn quota_refresh_status_has_one_visible_precedence() {
    let mut quota = QuotaSnapshot::default();
    assert_eq!(
        quota_refresh_status(AccountAuthState::Active, &quota, false),
        QuotaRefreshStatus::Pending
    );
    assert_eq!(
        quota_refresh_status(AccountAuthState::Active, &quota, true),
        QuotaRefreshStatus::Refreshing
    );
    quota.updated_at_ms = Some(1);
    assert_eq!(
        quota_refresh_status(AccountAuthState::Active, &quota, false),
        QuotaRefreshStatus::Updated
    );
    assert_eq!(
        quota_refresh_status(
            AccountAuthState::RequiresReauth(crate::accounts::ReauthReason::InvalidGrant),
            &quota,
            true,
        ),
        QuotaRefreshStatus::RequiresReauth
    );
}
