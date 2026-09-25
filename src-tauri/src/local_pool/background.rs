use super::{
    accounts::{
        quota_refresh::{
            prepare_account_credentials, refresh_account_models_once, refresh_account_quota_once,
            AccountQuotaOutcome, AccountQuotaRefreshResponse,
        },
        wake::{completion_from_execution, execute_with_runtime, CodexWakeClient},
    },
    error::{ErrorCode, LocalPoolError, Result},
    state::DesktopState,
};
use std::{collections::BTreeSet, time::Duration};
use tauri::{AppHandle, Emitter, Manager};
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    automations::{
        verify_wake_countdown, WakeCompletion, WakeCompletionOutcome, WakePermit,
        WakeVerificationOutcome,
    },
    pricing::pricing_refresh_delay,
    providers::chatgpt::{
        configure_codex_client_version, refresh_codex_client_release,
        CODEX_RELEASE_REFRESH_INTERVAL,
    },
    unix_time_ms as current_time_ms,
};

mod timing;
mod wake_policy;

use timing::{due_wait, DueWait};
pub(super) use wake_policy::codex_wake_policy;

const WAKE_BATCH_SIZE: usize = 2;
const WORKER_ERROR_RETRY_MS: u64 = 60_000;
const WAKE_VERIFICATION_DELAY_MS: u64 = 5_000;
const WAKE_OUTPUT_TOKEN_CAP: u16 = 8;

pub(crate) fn start(app: AppHandle) {
    crate::diagnostics::record_operation("background", "workers_started", &[]);
    let recovery_app = app.clone();
    let _ownership_recovery = tauri::async_runtime::spawn(async move {
        let state = recovery_app.state::<DesktopState>();
        if let Err(error) =
            super::commands::remote_server::recover_pending_remote_ownership(&state).await
        {
            crate::diagnostics::record_error(
                "background-remote-recovery",
                Some("recover_pending_failed"),
                &error.message,
                &[],
            );
        }
        if let Err(error) =
            super::commands::remote_server::reconcile_saved_remote_ownership(&state).await
        {
            crate::diagnostics::record_error(
                "background-remote-recovery",
                Some("reconcile_failed"),
                &error.message,
                &[],
            );
        }
    });
    if let Err(error) = app.state::<DesktopState>().start_refresh() {
        crate::diagnostics::record_error(
            "account-refresh",
            Some("start_failed"),
            &error.message,
            &[],
        );
    }
    let pricing_app = app.clone();
    let _pricing_worker = tauri::async_runtime::spawn(async move {
        pricing_loop(pricing_app).await;
    });
    let metadata_app = app.clone();
    let _metadata_worker = tauri::async_runtime::spawn(async move {
        model_metadata_loop(metadata_app).await;
    });
    let codex_release_app = app.clone();
    let _codex_release_worker = tauri::async_runtime::spawn(async move {
        codex_release_loop(codex_release_app).await;
    });
    let _wake_worker = tauri::async_runtime::spawn(async move {
        wake_loop(app).await;
    });
}

/// Keeps Relay's own OAuth identity aligned with the newest published Rust
/// Codex release. This is intentionally independent of pool activity: a
/// paused or empty pool must still pick up a newer identity for its next use.
async fn codex_release_loop(_app: AppHandle) {
    loop {
        match refresh_codex_client_release().await {
            Ok(release) => {
                if !configure_codex_client_version(release.version()) {
                    crate::diagnostics::record_error(
                        "background-codex-release",
                        Some("configure_failed"),
                        "published Codex release could not be applied",
                        &[],
                    );
                }
            }
            Err(error) => crate::diagnostics::record_error(
                "background-codex-release",
                Some("refresh_failed"),
                &error.to_string(),
                &[],
            ),
        }
        tokio::time::sleep(CODEX_RELEASE_REFRESH_INTERVAL).await;
    }
}

async fn model_metadata_loop(app: AppHandle) {
    let instance_id = app
        .state::<DesktopState>()
        .root
        .to_string_lossy()
        .into_owned();
    loop {
        let loader = app.state::<DesktopState>().model_metadata_loader();
        let now_ms = current_time_ms();
        let delay =
            pricing_refresh_delay(&instance_id, loader.next_refresh_deadline(now_ms), now_ms);
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = loader.wait_for_schedule_change() => continue,
        }
        let loader = app.state::<DesktopState>().model_metadata_loader();
        if loader.refresh_due(current_time_ms()) {
            if let Err(error) = loader.refresh(false).await {
                crate::diagnostics::record_error(
                    "background-model-metadata",
                    Some("refresh_failed"),
                    &error.to_string(),
                    &[],
                );
            }
            let _ = app.emit("zenith-state-changed", ());
        }
    }
}

async fn pricing_loop(app: AppHandle) {
    let instance_id = app
        .state::<DesktopState>()
        .root
        .to_string_lossy()
        .into_owned();
    loop {
        let state = app.state::<DesktopState>();
        let loader = state.pricing_loader();
        let now_ms = current_time_ms();
        let deadline = loader.next_refresh_deadline(now_ms);
        let delay = pricing_refresh_delay(&instance_id, deadline, now_ms);
        tokio::select! {
            _ = tokio::time::sleep(delay) => {}
            _ = loader.wait_for_schedule_change() => continue,
        }

        let state = app.state::<DesktopState>();
        let loader = state.pricing_loader();
        if loader.refresh_due(current_time_ms()) {
            if let Err(error) = loader.refresh(false).await {
                crate::diagnostics::record_error(
                    "background-pricing",
                    Some("refresh_failed"),
                    &error.to_string(),
                    &[],
                );
            }
            // Refresh failures update the catalog status too. Notify the UI
            // for every attempt so snapshot consumers can recalculate derived
            // pricing data without waiting for another background event.
            let _ = app.emit("zenith-state-changed", ());
        }
    }
}

/// Start a one-shot model discovery for accounts that have just become local
/// pool members.
///
/// A membership change needs immediate model discovery, otherwise the UI can
/// show a fresh quota together with an empty model list until the user
/// manually refreshes it. Keep this one-shot work off the command path and
/// join the independent model-kind job in the common refresh service.
pub(crate) fn refresh_account_models_in_background(app: AppHandle, account_ids: Vec<String>) {
    let account_ids = account_ids
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if account_ids.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let state = app.state::<DesktopState>();
        for account_id in account_ids {
            refresh_account_models_and_notify(&state, &app, &account_id).await;
        }
    });
}

/// Runs one model discovery attempt and publishes both its result and a
/// redacted diagnostic consistently for manual and scheduled refreshes.
async fn refresh_account_models_and_notify(
    state: &DesktopState,
    app: &AppHandle,
    account_id: &str,
) {
    if let Err(error) = refresh_account_models_once(state, account_id).await {
        crate::diagnostics::record_error(
            "background-account-models",
            Some("refresh_failed"),
            &error.message,
            &[("account", crate::diagnostics::hash_identifier(account_id))],
        );
    }
    // The model list and any persisted discovery error are both part of the
    // runtime snapshot, so notify the frontend after each account rather than
    // waiting for a bulk operation to finish.
    let _ = app.emit("zenith-state-changed", ());
}

pub(crate) async fn run_due_confirmation_wakes(
    state: &DesktopState,
    max_claims: usize,
) -> Result<usize> {
    let permits = match state
        .claim_due_confirmation_wakes(current_time_ms(), max_claims.min(WAKE_BATCH_SIZE))
    {
        Ok(permits) => permits,
        Err(error) => {
            crate::diagnostics::record_error(
                "background-wake",
                Some("claim_confirmation_failed"),
                &error.message,
                &[],
            );
            return Err(error);
        }
    };
    run_wake_permits(state, permits).await
}

async fn wake_loop(app: AppHandle) {
    loop {
        let state = app.state::<DesktopState>();
        state.wait_for_background_session_active().await;
        let wait_result = tokio::select! {
            _ = state.wait_for_background_session_inactive() => continue,
            result = wait_for_automatic_wake(&state) => result,
        };
        if let Err(error) = wait_result {
            crate::diagnostics::record_error(
                "background-wake",
                Some("schedule_failed"),
                &error.message,
                &[],
            );
            tokio::time::sleep(Duration::from_millis(WORKER_ERROR_RETRY_MS)).await;
            continue;
        }
        if !state.background_session_active() {
            continue;
        }
        let permits = match app
            .state::<DesktopState>()
            .claim_due_automatic_wakes(current_time_ms(), WAKE_BATCH_SIZE)
        {
            Ok(permits) => permits,
            Err(error) => {
                crate::diagnostics::record_error(
                    "background-wake",
                    Some("claim_failed"),
                    &error.message,
                    &[],
                );
                tokio::time::sleep(Duration::from_millis(WORKER_ERROR_RETRY_MS)).await;
                continue;
            }
        };
        let state = app.state::<DesktopState>();
        let result = run_wake_permits(&state, permits).await;
        if let Err(error) = result {
            crate::diagnostics::record_error(
                "background-wake",
                Some("execution_failed"),
                &error.message,
                &[],
            );
            tokio::time::sleep(Duration::from_millis(WORKER_ERROR_RETRY_MS)).await;
        }
    }
}

async fn wait_for_automatic_wake(state: &DesktopState) -> Result<()> {
    match due_wait(state.next_automatic_wake_due()?, current_time_ms()) {
        DueWait::Ready => {}
        DueWait::Notify => state.wait_for_wake().await,
        DueWait::Sleep(delay) => {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = state.wait_for_wake() => {},
            }
        }
    }
    Ok(())
}

async fn run_wake_permits(state: &DesktopState, permits: Vec<WakePermit>) -> Result<usize> {
    let claimed = permits.len();
    let mut first_error = None;
    for permit in permits {
        let account_hash = crate::diagnostics::hash_identifier(&permit.account_id);
        match execute_wake_permit(state, &permit).await {
            Ok(Some(completion)) => {
                if let Err(error) = state.complete_wake(permit, completion) {
                    crate::diagnostics::record_error(
                        "background-wake",
                        Some("complete_failed"),
                        &error.message,
                        &[("account", account_hash.clone())],
                    );
                    first_error.get_or_insert(error);
                }
            }
            Ok(None) => {}
            Err(error) => {
                crate::diagnostics::record_error(
                    "background-wake",
                    Some("permit_failed"),
                    &error.message,
                    &[("account", account_hash)],
                );
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(claimed), Err)
}

async fn execute_wake_permit(
    state: &DesktopState,
    permit: &WakePermit,
) -> Result<Option<WakeCompletion>> {
    if !state.is_wake_permit_active(permit)? {
        return Ok(None);
    }
    let execution = if let Some(runtime) = state.gateway.runtime().await {
        // Keep scheduler-owned wake traffic on the same live runtime as user
        // requests.  The runtime receives an account-only scope, refreshes the
        // token through its authority, and records the attempt in the shared
        // usage/diagnostics pipeline.
        crate::diagnostics::breadcrumb(
            "background-wake",
            "runtime_execution_started",
            &[(
                "account",
                crate::diagnostics::hash_identifier(&permit.account_id),
            )],
        );
        execute_with_runtime(
            runtime,
            super::commands::pool::SYSTEM_GATEWAY_KEY_ID,
            &permit.request,
        )
        .await
    } else {
        // Startup can schedule the worker before the optional local listener
        // has been created.  Preserve the existing direct probe only for that
        // narrow window; once a runtime exists all wakes use the shared path.
        crate::diagnostics::breadcrumb(
            "background-wake",
            "runtime_unavailable_direct_fallback",
            &[(
                "account",
                crate::diagnostics::hash_identifier(&permit.account_id),
            )],
        );
        let prepared = match prepare_account_credentials(state, &permit.account_id).await {
            Ok(prepared) => prepared,
            Err(error) => {
                crate::diagnostics::record_error(
                    "background-wake",
                    Some(credential_error_code(&error)),
                    &error.message,
                    &[(
                        "account",
                        crate::diagnostics::hash_identifier(&permit.account_id),
                    )],
                );
                return Ok(Some(failed_wake_completion(credential_error_code(&error))));
            }
        };
        let client = match CodexWakeClient::new_with_proxy(
            prepared.tokens().access_token(),
            prepared.provider_account_id(),
            prepared.proxy(),
        ) {
            Ok(client) => client,
            Err(failure) => {
                crate::diagnostics::record_error(
                    "background-wake",
                    Some("client_create_failed"),
                    &failure.to_string(),
                    &[(
                        "account",
                        crate::diagnostics::hash_identifier(&permit.account_id),
                    )],
                );
                return Ok(Some(completion_from_execution(
                    &Err(failure),
                    WakeVerificationOutcome::Unconfirmed,
                    current_time_ms(),
                )));
            }
        };
        client.execute(&permit.request).await
    };
    if !state.is_wake_permit_active(permit)? {
        return Ok(None);
    }
    let verification = if execution.is_ok() {
        tokio::time::sleep(Duration::from_millis(permit.verification_delay_ms)).await;
        if !state.is_wake_permit_active(permit)? {
            return Ok(None);
        }
        match refresh_account_quota_once(state, &permit.account_id).await {
            Ok(response) => verification_from_refresh(permit, &response),
            Err(error) => {
                crate::diagnostics::record_error(
                    "background-wake",
                    Some("verification_refresh_failed"),
                    &error.message,
                    &[(
                        "account",
                        crate::diagnostics::hash_identifier(&permit.account_id),
                    )],
                );
                WakeVerificationOutcome::Unconfirmed
            }
        }
    } else {
        WakeVerificationOutcome::Unconfirmed
    };
    Ok(Some(completion_from_execution(
        &execution,
        verification,
        current_time_ms(),
    )))
}

fn verification_from_refresh(
    permit: &WakePermit,
    response: &AccountQuotaRefreshResponse,
) -> WakeVerificationOutcome {
    if !matches!(&response.quota, AccountQuotaOutcome::Updated { .. }) {
        return WakeVerificationOutcome::Unconfirmed;
    }
    verify_wake_countdown(
        permit.verification.baseline_window.as_ref(),
        response
            .account
            .account
            .quota
            .window(permit.verification.window_kind),
    )
}

fn failed_wake_completion(code: &'static str) -> WakeCompletion {
    WakeCompletion {
        outcome: WakeCompletionOutcome::Failed,
        completed_at_ms: current_time_ms(),
        latency_ms: None,
        input_tokens: None,
        output_tokens: None,
        error_code: Some(code.to_string()),
    }
}

fn credential_error_code(error: &LocalPoolError) -> &'static str {
    match &error.code {
        ErrorCode::NotFound => error_codes::WAKE_ACCOUNT_MISSING,
        _ => error_codes::WAKE_CREDENTIALS_UNAVAILABLE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local_pool::models::LocalAccountRecord;
    use std::collections::BTreeSet;
    use zenith_relay_core::{
        accounts::{
            AccountAuthMode, AccountAuthState, AccountHealthState, AccountIdentity, AccountRecord,
        },
        automations::{WakeExecutionRequest, WakeTrigger, WakeVerificationMetadata},
        quota::{
            QuotaAdapterCapabilities, QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription,
        },
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
}
