use super::*;
use crate::accounts::{AccountAuthState, ReauthReason, TokenSet};

fn account_event(success: bool, category: Option<&str>) -> UsageEvent {
    UsageEvent {
        request_id: "synthetic-observation".into(),
        attempt: 1,
        local_key_id: "key-1".into(),
        source_id: "openai-codex".into(),
        candidate_id: Some("account-1".into()),
        account_id: Some("account-1".into()),
        account_token_generation: Some(1),
        client_context_id: None,
        routing: None,
        requested_model: Some("gpt-test".into()),
        resolved_model: Some("gpt-test".into()),
        requested_reasoning_effort: None,
        effective_reasoning_effort: None,
        wire_api: WireApi::Responses,
        transport: crate::UsageTransport::Http,
        service_tier: DefaultServiceTier::Standard,
        applied_service_tier: None,
        success,
        http_status: if success { 200 } else { 401 },
        error_category: category.map(str::to_owned),
        tool_use: ToolUseDiagnostics::default(),
        cooldown_scope: None,
        retry_at_ms: None,
        consecutive_failures: None,
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
        image_usage: None,
    }
}

fn register(runtime: &GatewayRuntime, generation: u64, state: AccountAuthState) {
    runtime.chatgpt_accounts["account-1"]
        .token_authority
        .register_if_absent(
            "account-1",
            TokenSet::new("synthetic-access", None, None, None, 1, generation).unwrap(),
            state,
        )
        .unwrap();
}

fn health(runtime: &GatewayRuntime) -> CandidateHealth {
    runtime
        .lock_scheduler()
        .candidate("account-1")
        .unwrap()
        .health
}

#[test]
fn stale_results_cannot_change_runtime_health_or_quota() {
    for (success, category) in [
        (true, None),
        (false, Some(crate::error_codes::UPSTREAM_UNAUTHORIZED)),
        (false, Some(crate::error_codes::UPSTREAM_ACCOUNT_DISABLED)),
        (false, Some(crate::error_codes::UPSTREAM_QUOTA_EXHAUSTED)),
    ] {
        let runtime = quota_runtime(QuotaSnapshot::default());
        register(&runtime, 2, AccountAuthState::Active);
        let original_health = if success {
            CandidateHealth::Blocked
        } else {
            CandidateHealth::Healthy
        };
        runtime.set_candidate_health("account-1", original_health);
        let before = crate::poison::mutex(&runtime.passive_quotas)["account-1"]
            .snapshot
            .clone();
        let mut event = account_event(success, category);
        event.quota_snapshot = Some(QuotaSnapshot {
            limit_reached: true,
            updated_at_ms: Some(2_000),
            ..QuotaSnapshot::default()
        });
        assert!(runtime.apply_usage_event(&event, 2_000).is_none());
        assert_eq!(health(&runtime), original_health);
        assert_eq!(
            crate::poison::mutex(&runtime.passive_quotas)["account-1"].snapshot,
            before
        );
        assert!(
            runtime
                .take_passive_quota_snapshot("account-1", 2_001)
                .is_none()
        );
    }
}

#[test]
fn request_results_do_not_clear_terminal_runtime_authentication_state() {
    for reason in [
        ReauthReason::InvalidGrant,
        ReauthReason::TokenRotationRecoveryRequired,
    ] {
        for success in [true, false] {
            let runtime = quota_runtime(QuotaSnapshot::default());
            register(&runtime, 1, AccountAuthState::RequiresReauth(reason));
            runtime.set_candidate_health("account-1", CandidateHealth::ReauthRequired);
            let category = (!success).then_some(crate::error_codes::UPSTREAM_ACCOUNT_DISABLED);
            let _ = runtime.apply_usage_event(&account_event(success, category), 2_000);
            assert_eq!(health(&runtime), CandidateHealth::ReauthRequired);
        }
    }
}

#[test]
fn current_generation_success_still_recovers_runtime_health() {
    for state in [
        AccountAuthState::Active,
        AccountAuthState::RequiresReauth(ReauthReason::ReusedRefreshToken),
    ] {
        let runtime = quota_runtime(QuotaSnapshot::default());
        register(&runtime, 1, state);
        runtime.set_candidate_health("account-1", CandidateHealth::Degraded);
        let _ = runtime.apply_usage_event(&account_event(true, None), 2_000);
        assert_eq!(health(&runtime), CandidateHealth::Healthy);
    }
}

#[test]
fn stale_http_and_websocket_headers_cannot_exhaust_current_quota() {
    for status in [
        reqwest::StatusCode::OK,
        reqwest::StatusCode::SWITCHING_PROTOCOLS,
        reqwest::StatusCode::TOO_MANY_REQUESTS,
    ] {
        let runtime = quota_runtime(QuotaSnapshot::default());
        register(&runtime, 2, AccountAuthState::Active);
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-codex-primary-used-percent", "100".parse().unwrap());
        assert!(!runtime.observe_codex_quota_headers(
            "account-1",
            Some(1),
            status,
            &headers,
            2_000
        ));
        assert_eq!(
            crate::poison::mutex(&runtime.passive_quotas)["account-1"].snapshot,
            QuotaSnapshot::default()
        );
        assert!(runtime.observe_codex_quota_headers("account-1", Some(2), status, &headers, 2_000));
        let current = runtime
            .take_passive_quota_snapshot("account-1", 2_001)
            .unwrap();
        assert_eq!(current.primary.unwrap().available_basis_points, Some(0));
    }
}

#[test]
fn stale_completion_cannot_consume_pending_current_quota() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    register(&runtime, 2, AccountAuthState::Active);
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("x-codex-primary-used-percent", "100".parse().unwrap());
    assert!(runtime.observe_codex_quota_headers(
        "account-1",
        Some(2),
        reqwest::StatusCode::OK,
        &headers,
        2_000
    ));
    assert!(
        runtime
            .apply_usage_event(&account_event(true, None), 2_001)
            .is_none()
    );
    let current = runtime
        .take_passive_quota_snapshot("account-1", 2_001)
        .unwrap();
    assert_eq!(current.primary.unwrap().available_basis_points, Some(0));
}

#[test]
fn removed_authority_cannot_be_recovered_by_a_late_success() {
    let runtime = quota_runtime(QuotaSnapshot::default());
    register(&runtime, 1, AccountAuthState::Active);
    runtime.set_candidate_health("account-1", CandidateHealth::Blocked);
    assert!(
        runtime.chatgpt_accounts["account-1"]
            .token_authority
            .remove("account-1")
    );
    assert!(
        runtime
            .apply_usage_event(&account_event(true, None), 2_000)
            .is_none()
    );
    assert_eq!(health(&runtime), CandidateHealth::Blocked);
}
