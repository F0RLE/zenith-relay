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
#[cfg(test)]
use wake_worker::{execute_wake_permit, verification_from_refresh};

const WAKE_BATCH_SIZE: usize = 2;
const WORKER_ERROR_RETRY_MS: u64 = 60_000;
const WAKE_VERIFICATION_DELAY_MS: u64 = 5_000;
const WAKE_OUTPUT_TOKEN_CAP: u16 = 8;

pub(crate) fn start(app: AppHandle) {
    crate::diagnostics::record_operation("background", "workers_started", &[]);
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
    let _codex_release_worker = tauri::async_runtime::spawn(codex_release_loop());
    let maintenance_app = app.clone();
    let _maintenance_worker = tauri::async_runtime::spawn(async move {
        loops::usage_maintenance_loop(maintenance_app).await;
    });
    let _wake_worker = tauri::async_runtime::spawn(async move {
        wake_worker::wake_loop(app).await;
    });
}
#[cfg(test)]
mod tests;
