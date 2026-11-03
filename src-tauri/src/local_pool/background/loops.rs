use super::super::{accounts::quota_refresh::refresh_account_models_once, state::DesktopState};
use std::collections::BTreeSet;
use tauri::{AppHandle, Emitter, Manager};
use zenith_relay_core::{
    pricing::pricing_refresh_delay,
    providers::chatgpt::{
        CODEX_RELEASE_REFRESH_INTERVAL, configure_codex_client_version,
        refresh_codex_client_release,
    },
    unix_time_ms as current_time_ms,
};

/// Keeps Relay's own OAuth identity aligned with the newest published Rust
/// Codex release. This is intentionally independent of pool activity: a
/// paused or empty pool must still pick up a newer identity for its next use.
pub(super) async fn codex_release_loop() {
    loop {
        match refresh_codex_client_release().await {
            Ok(release) => {
                let _ = configure_codex_client_version(release.version());
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

pub(super) async fn usage_maintenance_loop(app: AppHandle) {
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let state = app.state::<DesktopState>().inner().clone();
        match tauri::async_runtime::spawn_blocking(move || {
            zenith_relay_core::gateway::release_expired_cache_entries();
            state.telemetry.maintain_usage()
        })
        .await
        {
            Ok(Ok(removed)) if removed > 0 => {
                let _ = app.emit("zenith-usage-recorded", ());
                let _ = app.emit("zenith-state-changed", ());
            }
            Ok(Ok(_)) => {}
            _ => crate::diagnostics::record_error(
                "usage-maintenance",
                Some("retention_failed"),
                "Usage history maintenance did not complete",
                &[],
            ),
        }
    }
}

pub(super) async fn model_metadata_loop(app: AppHandle) {
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
            // Reference metadata participates in historical pricing, unlike
            // ordinary account/quota state changes.
            let _ = app.emit("zenith-usage-recorded", ());
            let _ = app.emit("zenith-state-changed", ());
        }
    }
}

pub(super) async fn pricing_loop(app: AppHandle) {
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
            let _ = app.emit("zenith-usage-recorded", ());
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
