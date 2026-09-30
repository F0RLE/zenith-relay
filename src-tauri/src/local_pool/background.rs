#[cfg(test)]
use super::accounts::quota_refresh::{AccountQuotaOutcome, AccountQuotaRefreshResponse};
use super::state::DesktopState;
use tauri::{AppHandle, Manager};
#[cfg(test)]
use zenith_relay_core::automations::{WakePermit, WakeVerificationOutcome};

mod loops;
mod wake_policy;
mod wake_worker;

pub(crate) use loops::refresh_account_models_in_background;
use loops::{codex_release_loop, model_metadata_loop, pricing_loop};

pub(super) use wake_policy::codex_wake_policy;
pub(crate) use wake_worker::run_due_confirmation_wakes;
#[cfg(test)]
use wake_worker::{execute_wake_permit, verification_from_refresh};

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
        wake_worker::wake_loop(app).await;
    });
}
#[cfg(test)]
mod tests;
