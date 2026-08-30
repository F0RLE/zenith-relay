use super::{
    accounts::{
        quota_refresh::{
            next_quota_refresh_at, prepare_account_credentials, record_model_refresh_error,
            record_quota_refresh_error, refresh_account_models_once, refresh_account_quota_once,
            AccountQuotaOutcome, AccountQuotaRefreshResponse,
        },
        reset_credits::consume_local_reset_credit_for_account,
        wake::{completion_from_execution, CodexWakeClient},
    },
    error::{ErrorCode, LocalPoolError, Result},
    state::DesktopState,
};
use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::task::{Id as TaskId, JoinError, JoinSet};
use zenith_relay_core::{
    automations::{
        verify_wake_countdown, WakeCompletion, WakeCompletionOutcome, WakePermit, WakeTrigger,
        WakeVerificationOutcome,
    },
    providers::chatgpt::CodexQuotaClient,
    unix_time_ms as current_time_ms,
};

mod timing;
mod wake_policy;

use timing::{due_wait, DueWait};
use wake_policy::codex_wake_policy;

const QUOTA_BATCH_SIZE: usize = 5;
const WAKE_BATCH_SIZE: usize = 2;
const WORKER_ERROR_RETRY_MS: u64 = 60_000;
const WAKE_VERIFICATION_DELAY_MS: u64 = 5_000;
const WAKE_OUTPUT_TOKEN_CAP: u16 = 8;
const SOURCE_MODEL_REFRESH_START_DELAY_SECONDS: u64 = 5;
const ACCOUNT_MODEL_REFRESH_START_DELAY_SECONDS: u64 = 5;
const SOURCE_MODEL_REFRESH_INTERVAL_SECONDS: u64 = 8 * 60 * 60;
const ACCOUNT_MODEL_REFRESH_INTERVAL_SECONDS: u64 = 8 * 60 * 60;

pub(crate) fn start(app: AppHandle) {
    let recovery_app = app.clone();
    let _ownership_recovery = tauri::async_runtime::spawn(async move {
        let state = recovery_app.state::<DesktopState>();
        let _ = super::commands::remote_server::recover_pending_remote_ownership(&state).await;
        let _ = super::commands::remote_server::reconcile_saved_remote_ownership(&state).await;
    });
    let quota_app = app.clone();
    let _quota_worker = tauri::async_runtime::spawn(async move {
        quota_loop(quota_app).await;
    });
    let source_app = app.clone();
    let _source_model_worker = tauri::async_runtime::spawn(async move {
        source_model_loop(source_app).await;
    });
    let account_model_app = app.clone();
    let _account_model_worker = tauri::async_runtime::spawn(async move {
        account_model_loop(account_model_app).await;
    });
    let _wake_worker = tauri::async_runtime::spawn(async move {
        wake_loop(app).await;
    });
}

/// Start a one-shot model discovery for accounts that have just become local
/// pool members.
///
/// Quota refreshes and model discovery intentionally have separate periodic
/// lifecycles. A membership change is the exception: the account must be
/// discoverable immediately, otherwise the UI shows a fresh quota together
/// with an empty model list until the user manually refreshes it. Keep the
/// network work off the command path and reuse the per-account lock shared by
/// the regular quota/model workers.
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
            let result = refresh_account_models_once(&state, &account_id).await;
            if let Err(error) = result {
                let _ = record_model_refresh_error(&state, &account_id, &error);
            }
            // The model list and any persisted discovery error are both part
            // of the runtime snapshot, so notify the frontend after each
            // account rather than waiting for a bulk operation to finish.
            let _ = app.emit("zenith-state-changed", ());
        }
    });
}

pub(crate) async fn run_due_confirmation_wakes(
    state: &DesktopState,
    max_claims: usize,
) -> Result<usize> {
    let permits =
        state.claim_due_confirmation_wakes(current_time_ms(), max_claims.min(WAKE_BATCH_SIZE))?;
    run_wake_permits(state, permits).await
}

async fn quota_loop(app: AppHandle) {
    loop {
        let state = app.state::<DesktopState>();
        state.wait_for_background_session_active().await;
        let wait_result = tokio::select! {
            _ = state.wait_for_background_session_inactive() => continue,
            result = wait_for_quota_due(&state) => result,
        };
        if wait_result.is_err() {
            tokio::time::sleep(Duration::from_millis(WORKER_ERROR_RETRY_MS)).await;
            continue;
        }
        if !state.background_session_active() {
            continue;
        }
        if run_due_quota_refreshes(&app, false).await.is_err() {
            tokio::time::sleep(Duration::from_millis(WORKER_ERROR_RETRY_MS)).await;
        }
    }
}

async fn account_model_loop(app: AppHandle) {
    tokio::time::sleep(Duration::from_secs(
        ACCOUNT_MODEL_REFRESH_START_DELAY_SECONDS,
    ))
    .await;
    loop {
        let state = app.state::<DesktopState>();
        state.wait_for_background_session_active().await;
        let account_ids = state.store().map(|store| {
            store
                .accounts()
                .iter()
                .filter(|account| {
                    account.remote_location.is_none()
                        && account.account.is_automatic_quota_monitoring_eligible()
                })
                .map(|account| account.account.id.clone())
                .collect::<Vec<_>>()
        });
        if let Ok(account_ids) = account_ids {
            for account_id in account_ids {
                if !state.background_session_active() {
                    break;
                }
                let result = super::accounts::quota_refresh::refresh_account_models_once(
                    &state,
                    &account_id,
                )
                .await;
                if let Err(error) = result {
                    let _ = record_model_refresh_error(&state, &account_id, &error);
                }
                let _ = app.emit("zenith-state-changed", ());
            }
        }
        tokio::select! {
            _ = state.wait_for_background_session_inactive() => {},
            _ = tokio::time::sleep(Duration::from_secs(ACCOUNT_MODEL_REFRESH_INTERVAL_SECONDS)) => {},
        }
    }
}

async fn source_model_loop(app: AppHandle) {
    tokio::time::sleep(Duration::from_secs(
        SOURCE_MODEL_REFRESH_START_DELAY_SECONDS,
    ))
    .await;
    loop {
        let state = app.state::<DesktopState>();
        state.wait_for_background_session_active().await;
        let source_ids = {
            state.store().map(|store| {
                store
                    .sources()
                    .iter()
                    .filter(|source| source.enabled)
                    .map(|source| source.id.clone())
                    .collect::<Vec<_>>()
            })
        };
        if let Ok(source_ids) = source_ids {
            for source_id in source_ids {
                if !state.background_session_active() {
                    break;
                }
                let _ = super::commands::connections::refresh_local_source_models(
                    &state,
                    &source_id,
                    super::commands::connections::SourceRefreshMode::Background,
                )
                .await;
                let _ = app.emit("zenith-state-changed", ());
            }
            if !state.background_session_active() {
                continue;
            }
            if let Some(runtime) = state.gateway.runtime().await {
                runtime.prefetch_source_model_metadata();
            }
            let _mutation = state.setup_guard().await;
            let result = super::commands::profiles::refresh_active_codex_catalog(&state).await;
            super::commands::record_catalog_refresh_result(&state, &result);
        }
        tokio::select! {
            _ = state.wait_for_background_session_inactive() => {},
            _ = tokio::time::sleep(Duration::from_secs(SOURCE_MODEL_REFRESH_INTERVAL_SECONDS)) => {},
        }
    }
}

async fn wake_loop(app: AppHandle) {
    loop {
        let state = app.state::<DesktopState>();
        state.wait_for_background_session_active().await;
        let wait_result = tokio::select! {
            _ = state.wait_for_background_session_inactive() => continue,
            result = wait_for_automatic_wake(&state) => result,
        };
        if wait_result.is_err() {
            tokio::time::sleep(Duration::from_millis(WORKER_ERROR_RETRY_MS)).await;
            continue;
        }
        if !state.background_session_active() {
            continue;
        }
        let Ok(permits) = app
            .state::<DesktopState>()
            .claim_due_automatic_wakes(current_time_ms(), WAKE_BATCH_SIZE)
        else {
            tokio::time::sleep(Duration::from_millis(WORKER_ERROR_RETRY_MS)).await;
            continue;
        };
        let state = app.state::<DesktopState>();
        let result = run_wake_permits(&state, permits).await;
        if result.is_err() {
            tokio::time::sleep(Duration::from_millis(WORKER_ERROR_RETRY_MS)).await;
        }
    }
}

async fn wait_for_quota_due(state: &DesktopState) -> Result<()> {
    match due_wait(state.next_quota_refresh_due()?, current_time_ms()) {
        DueWait::Ready => {}
        DueWait::Notify => state.wait_for_quota_refresh().await,
        DueWait::Sleep(delay) => {
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = state.wait_for_quota_refresh() => {},
            }
        }
    }
    Ok(())
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

async fn run_due_quota_refreshes(app: &AppHandle, refresh_models: bool) -> Result<usize> {
    let mut workers = JoinSet::new();
    let mut active_permits = HashMap::with_capacity(QUOTA_BATCH_SIZE);
    let mut claimed = claim_and_spawn_quota_workers(
        app,
        &mut workers,
        &mut active_permits,
        QUOTA_BATCH_SIZE,
        refresh_models,
    )?;
    if claimed > 0 {
        let _ = app.emit("zenith-state-changed", ());
    }

    let mut first_error = None;
    while let Some(joined) = workers.join_next_with_id().await {
        let worker_id = quota_worker_id(&joined);
        let Some(permit) = active_permits.remove(&worker_id) else {
            first_error.get_or_insert_with(|| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "quota refresh worker permit was lost",
                )
            });
            continue;
        };
        match joined {
            Ok((_, response)) => {
                let state = app.state::<DesktopState>();
                if let Err(error) = settle_quota_refresh(&state, permit, response).await {
                    first_error.get_or_insert(error);
                }
            }
            Err(_) => {
                let state = app.state::<DesktopState>();
                if let Err(error) = reschedule_failed_quota_worker(
                    &state,
                    permit,
                    current_time_ms().saturating_add(WORKER_ERROR_RETRY_MS),
                ) {
                    first_error.get_or_insert(error);
                    continue;
                }
                first_error.get_or_insert_with(|| {
                    LocalPoolError::new(ErrorCode::InvalidState, "quota refresh worker failed")
                });
            }
        }
        let _ = app.emit("zenith-state-changed", ());
        let open_slots = QUOTA_BATCH_SIZE.saturating_sub(workers.len());
        if open_slots == 0 {
            continue;
        }
        match claim_and_spawn_quota_workers(
            app,
            &mut workers,
            &mut active_permits,
            open_slots,
            refresh_models,
        ) {
            Ok(new_claims) => claimed = claimed.saturating_add(new_claims),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(claimed), Err)
}

fn claim_and_spawn_quota_workers(
    app: &AppHandle,
    workers: &mut JoinSet<Result<AccountQuotaRefreshResponse>>,
    active_permits: &mut HashMap<TaskId, zenith_relay_core::quota::QuotaRefreshPermit>,
    max_claims: usize,
    refresh_models: bool,
) -> Result<usize> {
    let state = app.state::<DesktopState>();
    if !state.background_session_active() {
        return Ok(0);
    }
    let permits = state.claim_due_quota_refreshes(current_time_ms(), max_claims)?;
    let claimed = permits.len();
    for permit in permits {
        let worker_app = app.clone();
        let account_id = permit.account_id.clone();
        let task = workers.spawn(async move {
            let state = worker_app.state::<DesktopState>();
            refresh_account_quota_once(&state, &account_id, false, refresh_models).await
        });
        active_permits.insert(task.id(), permit);
    }
    Ok(claimed)
}

fn quota_worker_id<T>(result: &std::result::Result<(TaskId, T), JoinError>) -> TaskId {
    match result {
        Ok((id, _)) => *id,
        Err(error) => error.id(),
    }
}

fn reschedule_failed_quota_worker(
    state: &DesktopState,
    permit: zenith_relay_core::quota::QuotaRefreshPermit,
    due_at_ms: u64,
) -> Result<()> {
    state.reschedule_quota_refresh(permit, due_at_ms)?;
    Ok(())
}

async fn settle_quota_refresh(
    state: &DesktopState,
    permit: zenith_relay_core::quota::QuotaRefreshPermit,
    response: Result<AccountQuotaRefreshResponse>,
) -> Result<()> {
    match response {
        Ok(response) => {
            if let Some(due_at_ms) = next_quota_refresh_at(&response, current_time_ms()) {
                state.reschedule_quota_refresh(permit, due_at_ms)?;
            } else {
                state.complete_quota_refresh(permit)?;
            }
            let _ = evaluate_updated_transitions(state, &response);
            evaluate_weekly_exhaustions(state, &response).await?;
        }
        Err(error) => {
            let account_id = permit.account_id.clone();
            let _ = record_quota_refresh_error(state, &account_id, &error, current_time_ms());
            if terminal_quota_refresh_error(state, &account_id, &error)? {
                state.complete_quota_refresh(permit)?;
            } else {
                state.reschedule_quota_refresh(
                    permit,
                    current_time_ms().saturating_add(WORKER_ERROR_RETRY_MS),
                )?;
            }
        }
    }
    Ok(())
}

fn terminal_quota_refresh_error(
    state: &DesktopState,
    account_id: &str,
    error: &LocalPoolError,
) -> Result<bool> {
    if matches!(error.code, ErrorCode::NotFound) {
        return Ok(true);
    }
    Ok(state
        .store()?
        .account(account_id)
        .is_none_or(|account| account.account.auth_state.requires_fresh_login()))
}

fn evaluate_updated_transitions(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
) -> Result<()> {
    let AccountQuotaOutcome::Updated { transitions, .. } = &response.quota else {
        return Ok(());
    };
    if transitions.is_empty() {
        return Ok(());
    }
    let capabilities = CodexQuotaClient::new()
        .map_err(|failure| LocalPoolError::new(ErrorCode::InvalidState, failure.code))?
        .capabilities();
    let policy = codex_wake_policy(&response.account, &capabilities);
    let tasks = state.store()?.automations().tasks.clone();
    let now_ms = current_time_ms();
    for transition in transitions {
        for task in &tasks {
            state.evaluate_wake_transition(
                task,
                &response.account.account,
                transition,
                &policy,
                now_ms,
            )?;
        }
    }
    Ok(())
}

async fn evaluate_weekly_exhaustions(
    state: &DesktopState,
    response: &AccountQuotaRefreshResponse,
) -> Result<()> {
    if response.exhaustion_transitions.is_empty()
        || response.account.remote_location.is_some()
        || response
            .account
            .account
            .quota
            .reset_credits_available
            .unwrap_or(0)
            == 0
    {
        return Ok(());
    }
    let tasks = state.store()?.automations().tasks.clone();
    let has_weekly_task = tasks.iter().any(|task| {
        task.enabled
            && task.trigger == WakeTrigger::Weekly
            && task.account_selector.matches(&response.account.account)
    });
    if !has_weekly_task {
        return Ok(());
    }
    for transition in &response.exhaustion_transitions {
        if transition.window_kind != zenith_relay_core::quota::QuotaWindowKind::Secondary
            || state
                .weekly_reset_was_applied(&response.account.account.id, &transition.fingerprint)?
        {
            continue;
        }
        if consume_local_reset_credit_for_account(state, &response.account.account.id)
            .await
            .is_ok()
        {
            state
                .mark_weekly_reset_applied(&response.account.account.id, &transition.fingerprint)?;
            break;
        }
    }
    Ok(())
}

async fn run_wake_permits(state: &DesktopState, permits: Vec<WakePermit>) -> Result<usize> {
    let claimed = permits.len();
    let mut first_error = None;
    for permit in permits {
        match execute_wake_permit(state, &permit).await {
            Ok(Some(completion)) => {
                if let Err(error) = state.complete_wake(permit, completion) {
                    first_error.get_or_insert(error);
                }
            }
            Ok(None) => {}
            Err(error) => {
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
    let prepared = match prepare_account_credentials(state, &permit.account_id).await {
        Ok(prepared) => prepared,
        Err(error) => return Ok(Some(failed_wake_completion(credential_error_code(&error)))),
    };
    let client = match CodexWakeClient::new_with_proxy(
        prepared.tokens().access_token(),
        prepared.provider_account_id(),
        prepared.proxy(),
    ) {
        Ok(client) => client,
        Err(failure) => {
            return Ok(Some(completion_from_execution(
                &Err(failure),
                WakeVerificationOutcome::Unconfirmed,
                current_time_ms(),
            )))
        }
    };
    if !state.is_wake_permit_active(permit)? {
        return Ok(None);
    }
    let execution = client.execute(&permit.request).await;
    if !state.is_wake_permit_active(permit)? {
        return Ok(None);
    }
    let verification = if execution.is_ok() {
        tokio::time::sleep(Duration::from_millis(permit.verification_delay_ms)).await;
        if !state.is_wake_permit_active(permit)? {
            return Ok(None);
        }
        match refresh_account_quota_once(state, &permit.account_id, false, false).await {
            Ok(response) => {
                let _ = settle_verification_quota(state, &permit.account_id, &response);
                let _ = evaluate_updated_transitions(state, &response);
                let _ = evaluate_weekly_exhaustions(state, &response).await;
                verification_from_refresh(permit, &response)
            }
            Err(_) => WakeVerificationOutcome::Unconfirmed,
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

fn settle_verification_quota(
    state: &DesktopState,
    account_id: &str,
    response: &AccountQuotaRefreshResponse,
) -> Result<()> {
    if let Some(due_at_ms) = next_quota_refresh_at(response, current_time_ms()) {
        state.sync_account_quota_refresh(account_id, due_at_ms)?;
    } else {
        state.remove_quota_refresh(account_id)?;
    }
    Ok(())
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
        ErrorCode::NotFound => "wake_account_missing",
        _ => "wake_credentials_unavailable",
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
            ReauthReason,
        },
        automations::{WakeExecutionRequest, WakeTrigger, WakeVerificationMetadata},
        quota::{
            QuotaAdapterCapabilities, QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription,
        },
        WireApi,
    };

    #[test]
    fn source_models_refresh_at_startup_then_every_eight_hours() {
        assert_eq!(SOURCE_MODEL_REFRESH_START_DELAY_SECONDS, 5);
        assert_eq!(SOURCE_MODEL_REFRESH_INTERVAL_SECONDS, 8 * 60 * 60);
    }

    #[test]
    fn account_models_refresh_is_independent_from_quota_schedule() {
        assert_eq!(ACCOUNT_MODEL_REFRESH_START_DELAY_SECONDS, 5);
        assert_eq!(ACCOUNT_MODEL_REFRESH_INTERVAL_SECONDS, 8 * 60 * 60);
    }

    #[test]
    fn only_reauthentication_stops_automatic_quota_retries() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-background-auth-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).unwrap();
        let mut account = account_record();
        state
            .store()
            .unwrap()
            .upsert_account(account.clone())
            .unwrap();
        let error = LocalPoolError::new(ErrorCode::InvalidState, "safe failure");
        assert!(!terminal_quota_refresh_error(&state, "account-1", &error).unwrap());

        account.account.auth_state = AccountAuthState::RequiresReauth(ReauthReason::InvalidGrant);
        state
            .store()
            .unwrap()
            .upsert_account(account.clone())
            .unwrap();
        assert!(terminal_quota_refresh_error(&state, "account-1", &error).unwrap());

        account.account.auth_state = AccountAuthState::DegradedAccessOnly;
        state.store().unwrap().upsert_account(account).unwrap();
        assert!(!terminal_quota_refresh_error(&state, "account-1", &error).unwrap());
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

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
    async fn panicked_and_canceled_quota_workers_reschedule_their_permits() {
        let root = std::env::temp_dir().join(format!(
            "zenith-relay-quota-worker-{}",
            uuid::Uuid::new_v4()
        ));
        let state = DesktopState::open(root.clone()).unwrap();
        state.mark_quota_refresh("account-panic", 100).unwrap();
        state.mark_quota_refresh("account-cancel", 100).unwrap();
        let mut permits = state.claim_due_quota_refreshes(100, 2).unwrap();
        assert_eq!(permits.len(), 2);

        let mut workers = JoinSet::new();
        let mut active_permits = HashMap::new();
        let panic_task = workers.spawn(async { panic!("synthetic quota worker panic") });
        active_permits.insert(panic_task.id(), permits.pop().unwrap());
        let canceled_task = workers.spawn(async { std::future::pending::<()>().await });
        active_permits.insert(canceled_task.id(), permits.pop().unwrap());
        canceled_task.abort();

        while let Some(joined) = workers.join_next_with_id().await {
            assert!(joined.is_err());
            let permit = active_permits
                .remove(&quota_worker_id(&joined))
                .expect("worker permit must remain recoverable");
            reschedule_failed_quota_worker(&state, permit, 200).unwrap();
        }

        assert!(active_permits.is_empty());
        let mut recovered = state
            .claim_due_quota_refreshes(200, 2)
            .unwrap()
            .into_iter()
            .map(|permit| permit.account_id)
            .collect::<Vec<_>>();
        recovered.sort();
        assert_eq!(recovered, vec!["account-cancel", "account-panic"]);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
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
