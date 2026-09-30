use super::*;
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::models::{LocalAccountRecord, LocalGatewayKeyRecord, ProviderSourceRecord};
use crate::local_pool::usage_writer::apply_account_usage_state;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::Ordering;
use zenith_relay_core::{
    accounts::{
        AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
    },
    automations::{
        AccountSelector, WakeAdapterPolicy, WakeCompletion, WakeDecision, WakeExecutionPolicy,
        WakeModel, WakeModelPolicy, WakeOutcome, WakeTask, WakeTrigger,
    },
    quota::{QuotaSnapshot, QuotaTransition, QuotaWindow, QuotaWindowKind, Subscription},
    scheduler::refresh::{
        service::{RefreshRegistration, RefreshResult},
        RefreshFreshness, RefreshKind, RefreshOutcome,
    },
    UsageEvent, WireApi,
};
mod access_blocks;
mod desktop_runtime;
mod wake_cycles;

struct MemorySecrets(HashMap<String, String>);

impl SecretLookup for MemorySecrets {
    fn load(&self, secret_ref: &str) -> Result<Option<String>> {
        Ok(self.0.get(secret_ref).cloned())
    }
}

fn key_record(id: &str) -> LocalGatewayKeyRecord {
    LocalGatewayKeyRecord {
        id: id.into(),
        label: id.into(),
        enabled: true,
        system: false,
        secret_ref: format!("key:{id}"),
        created_at: "2026-07-10T00:00:00Z".into(),
        last_used_at: None,
    }
}

fn temp_root(prefix: &str) -> PathBuf {
    std::env::temp_dir().join(format!("zenith-relay-{prefix}-{}", uuid::Uuid::new_v4()))
}

fn account_record(id: &str) -> LocalAccountRecord {
    LocalAccountRecord {
        account: AccountRecord {
            id: id.into(),
            label: id.into(),
            identity: AccountIdentity::from_hashed_parts(
                "openai",
                "chatgpt.com/backend-api/codex",
                &format!("identity-{id}"),
                &format!("secret-{id}"),
                "default",
                None,
            )
            .unwrap(),
            auth_mode: AccountAuthMode::OAuth,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            source_id: "openai_codex".into(),
            secret_refs: vec![format!("account:{id}")],
            subscription: Subscription::default(),
            quota: QuotaSnapshot {
                primary: Some(QuotaWindow {
                    kind: QuotaWindowKind::Primary,
                    provider_cycle_id: None,
                    window_start_ms: None,
                    available_basis_points: Some(10_000),
                    explicitly_full: Some(true),
                    reset_at_ms: Some(10_000),
                    window_minutes: Some(300),
                    observed_at_ms: 100,
                    full_transition_fingerprint: Some("cycle-1".into()),
                    exhaustion_transition_fingerprint: None,
                }),
                ..QuotaSnapshot::default()
            },
            token_generation: 1,
            token_updated_at_ms: Some(1),
            tags: BTreeSet::new(),
            enabled: true,
            in_pool: true,
            draining: false,
            created_at_ms: 1,
            last_used_at_ms: None,
            last_error_code: None,
        },
        provider_family: Some("openai".into()),
        purchase_cost_micro_usd: None,
        remote_location: None,
        wire_api: WireApi::Responses,
        models: vec!["gpt-test".into()],
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        cooldowns: Default::default(),
        consecutive_failures: 0,
        client_auth_status: None,
        last_client_login_redirect_at_ms: None,
    }
}

fn wake_task(id: &str, execution_policy: WakeExecutionPolicy) -> WakeTask {
    WakeTask {
        id: id.into(),
        name: id.into(),
        enabled: true,
        account_selector: AccountSelector::AllEligible,
        window_kinds: [QuotaWindowKind::Primary].into(),
        model_policy: WakeModelPolicy::LightestSupported,
        trigger: WakeTrigger::QuotaFull,
        fallback_schedule: None,
        execution_policy,
        jitter_seconds: 0,
        max_attempts_per_cycle: 1,
        created_at_ms: 1,
        updated_at_ms: 1,
    }
}

fn wake_transition() -> QuotaTransition {
    wake_transition_with_fingerprint("cycle-1")
}

fn wake_transition_with_fingerprint(fingerprint: &str) -> QuotaTransition {
    QuotaTransition {
        window_kind: QuotaWindowKind::Primary,
        fingerprint: fingerprint.into(),
        transitioned_at_ms: 100,
    }
}

fn wake_policy() -> WakeAdapterPolicy {
    WakeAdapterPolicy {
        windows_requiring_activity: [QuotaWindowKind::Primary].into(),
        models: vec![WakeModel {
            id: "gpt-test".into(),
            lightness_rank: 1,
            wake_capable: true,
        }],
        verification_delay_ms: 1_000,
        output_token_cap: 8,
    }
}

fn account_usage_event(request_id: &str, success: bool) -> UsageEvent {
    UsageEvent {
        request_id: request_id.into(),
        attempt: 1,
        local_key_id: "key-1".into(),
        source_id: "openai_codex".into(),
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
        success,
        http_status: if success { 200 } else { 500 },
        error_category: (!success).then(|| "upstream".into()),
        tool_use: zenith_relay_core::ToolUseDiagnostics::default(),
        cooldown_scope: (!success).then(|| "*".into()),
        retry_at_ms: (!success).then_some(60_000),
        consecutive_failures: Some(u32::from(!success)),
        latency_ms: 7,
        ttft_ms: None,
        generation_ms: None,
        input_tokens: success.then_some(2),
        cached_input_tokens: None,
        cache_write_input_tokens: None,
        cache_write_ttl: None,
        reasoning_tokens: None,
        output_tokens: success.then_some(3),
        total_tokens: success.then_some(5),
        upstream_error: None,
        quota_snapshot: None,
    }
}

fn account_status_event(
    account_id: &str,
    http_status: u16,
    cooldown_scope: Option<&str>,
    retry_at_ms: Option<u64>,
    consecutive_failures: u32,
) -> UsageEvent {
    UsageEvent {
        request_id: format!("req-{account_id}-{http_status}"),
        attempt: 1,
        local_key_id: "key-1".into(),
        source_id: "openai_codex".into(),
        candidate_id: Some(account_id.into()),
        account_id: Some(account_id.into()),
        account_token_generation: Some(1),
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
        http_status,
        error_category: Some("upstream_status".into()),
        tool_use: zenith_relay_core::ToolUseDiagnostics::default(),
        cooldown_scope: cooldown_scope.map(str::to_string),
        retry_at_ms,
        consecutive_failures: Some(consecutive_failures),
        latency_ms: 7,
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
    }
}

fn account_success_event(account_id: &str) -> UsageEvent {
    let mut event = account_status_event(account_id, 200, None, None, 0);
    event.success = true;
    event.error_category = None;
    event
}
