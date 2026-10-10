use super::super::{
    accounts::{
        quota_refresh::{
            prepare_account_credentials, refresh_account_quota_once, AccountQuotaOutcome,
            AccountQuotaRefreshResponse,
        },
        wake::{completion_from_execution, execute_with_runtime, CodexWakeClient},
    },
    error::{ErrorCode, LocalPoolError, Result},
    state::DesktopState,
};
use super::{WAKE_BATCH_SIZE, WORKER_ERROR_RETRY_MS};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    automations::{
        verify_wake_countdown, WakeCompletion, WakeCompletionOutcome, WakePermit,
        WakeVerificationOutcome,
    },
    unix_time_ms as current_time_ms,
};

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

pub(super) async fn wake_loop(app: AppHandle) {
    loop {
        let state = app.state::<DesktopState>();
        state.wait_for_background_session_active().await;
        let wait_result = tokio::select! {
            _ = state.wait_for_background_session_inactive() => continue,
            wake_wait_result = wait_for_automatic_wake(&state) => wake_wait_result,
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
        let wake_run_result = run_wake_permits(&state, permits).await;
        if let Err(error) = wake_run_result {
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

pub(super) async fn execute_wake_permit(
    state: &DesktopState,
    permit: &WakePermit,
) -> Result<Option<WakeCompletion>> {
    if !state.is_wake_permit_active(permit)? {
        return Ok(None);
    }
    let account_hash = crate::diagnostics::hash_identifier(&permit.account_id);
    let execution = if let Some(runtime) = state.gateway.runtime().await {
        // Keep scheduler-owned wake traffic on the same live runtime as user
        // requests.  The runtime receives an account-only scope, refreshes the
        // token through its authority, and records the attempt in the shared
        // usage/diagnostics pipeline.
        crate::diagnostics::breadcrumb(
            "background-wake",
            "runtime_execution_started",
            &[("account", account_hash.clone())],
        );
        execute_with_runtime(
            runtime,
            super::super::commands::pool::SYSTEM_GATEWAY_KEY_ID,
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
            &[("account", account_hash.clone())],
        );
        let prepared = match prepare_account_credentials(state, &permit.account_id).await {
            Ok(prepared) => prepared,
            Err(error) => {
                crate::diagnostics::record_error(
                    "background-wake",
                    Some(credential_error_code(&error)),
                    &error.message,
                    &[("account", account_hash.clone())],
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
                    &[("account", account_hash.clone())],
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
                    &[("account", account_hash)],
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

pub(super) fn verification_from_refresh(
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DueWait {
    Ready,
    Notify,
    Sleep(Duration),
}

fn due_wait(next_due_at_ms: Option<u64>, now_ms: u64) -> DueWait {
    match next_due_at_ms {
        None => DueWait::Notify,
        Some(due_at_ms) if due_at_ms <= now_ms => DueWait::Ready,
        Some(due_at_ms) => DueWait::Sleep(Duration::from_millis(due_at_ms - now_ms)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn due_wait_uses_deadline_or_notification_without_polling() {
        assert_eq!(due_wait(None, 100), DueWait::Notify);
        assert_eq!(due_wait(Some(100), 100), DueWait::Ready);
        assert_eq!(due_wait(Some(90), 100), DueWait::Ready);
        assert_eq!(
            due_wait(Some(150), 100),
            DueWait::Sleep(Duration::from_millis(50))
        );
    }
}
