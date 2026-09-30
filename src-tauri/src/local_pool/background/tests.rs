use super::*;
use crate::local_pool::models::LocalAccountRecord;
use std::collections::BTreeSet;
use zenith_relay_core::{
    accounts::{
        AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
    },
    automations::{WakeExecutionRequest, WakeTrigger, WakeVerificationMetadata},
    quota::{QuotaAdapterCapabilities, QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription},
    WireApi,
};

#[test]
fn codex_policy_uses_capability_windows_and_lightest_allowed_model() {
    let mut account = account_record();
    account.models = vec![
        "gpt-codex".into(),
        "gpt-codex-mini".into(),
        "gpt-codex-nano".into(),
        "gpt-excluded-mini".into(),
    ];
    account.excluded_models = vec!["GPT-EXCLUDED-MINI".into()];
    let capabilities = QuotaAdapterCapabilities {
        supports_quota: true,
        supports_subscription: true,
        supported_windows: BTreeSet::from([QuotaWindowKind::Primary]),
        wake_windows: BTreeSet::from([QuotaWindowKind::Secondary]),
    };

    let policy = codex_wake_policy(&account, &capabilities);
    assert_eq!(
        policy.windows_requiring_activity,
        BTreeSet::from([QuotaWindowKind::Secondary])
    );
    assert_eq!(policy.models.len(), 3);
    assert_eq!(
        policy
            .models
            .iter()
            .min_by_key(|model| model.lightness_rank)
            .unwrap()
            .id,
        "gpt-codex-nano"
    );
    assert_eq!(policy.output_token_cap, WAKE_OUTPUT_TOKEN_CAP);
    assert_eq!(policy.verification_delay_ms, WAKE_VERIFICATION_DELAY_MS);

    account.discovered_models = Some(vec!["gpt-discovered-mini".into()]);
    let discovered_policy = codex_wake_policy(&account, &capabilities);
    assert_eq!(
        discovered_policy
            .models
            .iter()
            .map(|model| model.id.as_str())
            .collect::<Vec<_>>(),
        vec!["gpt-discovered-mini"]
    );
}

#[test]
fn verification_uses_only_normalized_before_and_after_windows() {
    let permit = wake_permit(full_window(Some(10_000), 100));
    let mut response = quota_response(full_window(Some(20_000), 200));
    assert_eq!(
        verification_from_refresh(&permit, &response),
        WakeVerificationOutcome::ConfirmedCountdownAdvanced
    );
    response.quota = AccountQuotaOutcome::Failed {
        code: "quota_transport".into(),
        retryable: true,
    };
    assert_eq!(
        verification_from_refresh(&permit, &response),
        WakeVerificationOutcome::Unconfirmed
    );
}

#[tokio::test]
async fn inactive_wake_permit_is_skipped_before_credentials_or_http() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-inactive-wake-{}",
        uuid::Uuid::new_v4()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    assert!(
        execute_wake_permit(&state, &wake_permit(full_window(Some(10_000), 100)))
            .await
            .unwrap()
            .is_none()
    );
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

fn account_record() -> LocalAccountRecord {
    LocalAccountRecord {
        account: AccountRecord {
            id: "account-1".into(),
            label: "Account".into(),
            identity: AccountIdentity::from_hashed_parts(
                "openai",
                "chatgpt.com/backend-api/codex",
                "identity-hash",
                "secret-hash",
                "default",
                None,
            )
            .unwrap(),
            auth_mode: AccountAuthMode::OAuth,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            source_id: "openai_codex".into(),
            secret_refs: vec!["account:account-1".into()],
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
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
        models: Vec::new(),
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

fn full_window(reset_at_ms: Option<u64>, observed_at_ms: u64) -> QuotaWindow {
    QuotaWindow {
        kind: QuotaWindowKind::Primary,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(10_000),
        explicitly_full: Some(true),
        reset_at_ms,
        window_minutes: Some(300),
        observed_at_ms,
        full_transition_fingerprint: Some("cycle-1".into()),
        exhaustion_transition_fingerprint: None,
    }
}

fn wake_permit(baseline: QuotaWindow) -> WakePermit {
    WakePermit {
        cycle_key: "cycle-key".into(),
        task_id: "task-1".into(),
        account_id: "account-1".into(),
        window_kind: QuotaWindowKind::Primary,
        transition_fingerprint: "cycle-1".into(),
        model_id: "gpt-codex-mini".into(),
        trigger: WakeTrigger::QuotaFull,
        requires_confirmation: false,
        verification_delay_ms: 1,
        output_token_cap: 8,
        attempt: 1,
        due_at_ms: 100,
        reserved_at_ms: 100,
        request: WakeExecutionRequest {
            account_id: "account-1".into(),
            model_id: "gpt-codex-mini".into(),
            window_kind: QuotaWindowKind::Primary,
            output_token_cap: 8,
        },
        verification: WakeVerificationMetadata {
            window_kind: QuotaWindowKind::Primary,
            baseline_window: Some(baseline),
            verify_after_ms: 1,
        },
    }
}

fn quota_response(after: QuotaWindow) -> AccountQuotaRefreshResponse {
    let mut account = account_record();
    account.account.quota.primary = Some(after);
    AccountQuotaRefreshResponse {
        account,
        quota: AccountQuotaOutcome::Updated {
            transitions: Vec::new(),
            exhaustion_transitions: Vec::new(),
        },
        exhaustion_transitions: Vec::new(),
    }
}
