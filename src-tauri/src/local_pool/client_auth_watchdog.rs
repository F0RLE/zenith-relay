//! Observes the official Codex renderer through its loopback CDP endpoint.
//!
//! This is intentionally a small, evidence-based observer. It records only a
//! stable login-page signal for the account bound to the active Relay profile;
//! it never captures network bodies, page text, cookies, or credentials.

use reqwest::Client;
use serde_json::json;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

use super::{commands::current_time_ms, profiles::codex, state::DesktopState};
use crate::platform::default_codex_home;

mod cdp;

const POLL_INTERVAL: Duration = Duration::from_secs(5);
static STARTED: OnceLock<()> = OnceLock::new();

pub(crate) fn start(app: AppHandle) {
    if STARTED.set(()).is_err() {
        return;
    }
    tauri::async_runtime::spawn(async move { run(app).await });
}

async fn run(app: AppHandle) {
    let client = match Client::builder().timeout(cdp::CDP_TIMEOUT).build() {
        Ok(client) => client,
        Err(_) => return,
    };
    let mut observed_account: Option<String> = None;
    let mut login_streak = 0u8;
    let mut available_streak = 0u8;
    let mut persisted_status: Option<&'static str> = None;
    loop {
        let state = app.state::<DesktopState>();
        let account_id =
            codex::active_managed_account_id(&default_codex_home(), &state.profile_backup_root())
                .ok()
                .flatten();
        if account_id != observed_account {
            observed_account = account_id.clone();
            login_streak = 0;
            available_streak = 0;
            persisted_status = None;
        }
        let Some(account_id) = account_id else {
            tokio::time::sleep(POLL_INTERVAL).await;
            continue;
        };
        let Some(port) = cdp::remote_debugging_port() else {
            tokio::time::sleep(POLL_INTERVAL).await;
            continue;
        };
        let targets = cdp::query_targets(&client, port).await;
        let mut login_signal = false;
        let mut available_signal = false;
        for target in targets.iter().filter(|target| target.is_codex_app_page()) {
            if let Some(snapshot) = cdp::query_snapshot(target).await {
                available_signal = true;
                login_signal |= snapshot.login_signal();
            }
        }
        if login_signal {
            login_streak = login_streak.saturating_add(1);
            available_streak = 0;
        } else if available_signal {
            available_streak = available_streak.saturating_add(1);
            login_streak = 0;
        } else {
            login_streak = 0;
            available_streak = 0;
        }
        let stable_status = if login_streak >= 2 {
            Some("login_required")
        } else if available_streak >= 2 {
            Some("available")
        } else {
            None
        };
        if let Some(status) = stable_status {
            if persisted_status != Some(status) {
                // Profile switches can race a slow CDP snapshot. Re-read the
                // active binding before persisting so a login page from the
                // newly selected profile cannot be attributed to the account
                // that was active at the beginning of this poll.
                let current_account_id = codex::active_managed_account_id(
                    &default_codex_home(),
                    &state.profile_backup_root(),
                )
                .ok()
                .flatten();
                if current_account_id.as_deref() != Some(account_id.as_str()) {
                    observed_account = current_account_id;
                    login_streak = 0;
                    available_streak = 0;
                    persisted_status = None;
                    tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
                let redirect_at_ms = if status == "login_required" {
                    Some(current_time_ms())
                } else {
                    state.store().ok().and_then(|store| {
                        store
                            .account(&account_id)
                            .and_then(|a| a.last_client_login_redirect_at_ms)
                    })
                };
                let observation = state.store().and_then(|mut store| {
                    store.update_client_auth_observation(
                        &account_id,
                        Some(status.to_string()),
                        redirect_at_ms,
                    )
                });
                if let Ok(changed) = observation {
                    if changed {
                        let _ = app.emit(
                            "zenith-state-changed",
                            json!({"reason": "client-auth-observation", "accountId": account_id, "status": status}),
                        );
                    }
                    // `Ok(false)` means the desired observation was already
                    // durable (or its account was deleted); only an actual
                    // store error must remain retryable on the next poll.
                    persisted_status = Some(status);
                }
            }
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

#[cfg(test)]
fn observation_write_completed<E>(write_result: &std::result::Result<bool, E>) -> bool {
    write_result.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_retries_failed_observation_persistence() {
        assert!(observation_write_completed(&Ok::<bool, ()>(false)));
        assert!(!observation_write_completed(&Err::<bool, ()>(())));
    }
}
