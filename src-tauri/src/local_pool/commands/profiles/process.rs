use crate::{
    launcher::{launch_codex_with_profile, stop_codex_and_wait},
    local_pool::{
        accounts::quota_refresh::sync_managed_account_profile,
        error::{CommandError, ErrorCode, LocalPoolError},
        profiles::codex,
        state::DesktopState,
    },
    platform::default_codex_home,
};
use std::future::Future;

pub(super) async fn launch_after_catalog_refresh<T>(
    refresh_required: bool,
    refresh: impl Future<Output = crate::local_pool::error::Result<T>>,
    report: impl FnOnce(&crate::local_pool::error::Result<T>),
    launch: impl FnOnce() -> Result<(), String>,
) -> Result<(), CommandError> {
    // A successful attach already installed the catalog under the setup guard.
    // Only a deferred or failed update needs another fetch before startup.
    if refresh_required {
        let refreshed = refresh.await;
        report(&refreshed);
    }
    // Catalog refresh has its own warning; failure must not strand a verified
    // profile after the caller explicitly stopped the previous client.
    launch().map_err(|error| {
        LocalPoolError::new(ErrorCode::Io, format!("failed to launch ChatGPT: {error}")).into()
    })
}

pub(super) fn stop_codex_for_profile_change() -> Result<bool, CommandError> {
    stop_codex_and_wait().map_err(|error| {
        LocalPoolError::new(
            ErrorCode::Io,
            format!("failed to stop ChatGPT before changing its profile: {error}"),
        )
        .into()
    })
}

pub(super) async fn stop_codex_and_sync_account(
    state: &DesktopState,
) -> Result<bool, CommandError> {
    stop_codex_and_sync_account_at(state, &default_codex_home()).await
}

pub(super) async fn stop_codex_and_sync_account_at(
    state: &DesktopState,
    profile_dir: &std::path::Path,
) -> Result<bool, CommandError> {
    let stopped = stop_codex_for_profile_change()?;
    let result: Result<(), CommandError> = async {
        if let Some(account_id) =
            codex::active_managed_account_id(profile_dir, &state.profile_backup_root())?
        {
            if state.store()?.account(&account_id).is_some() {
                sync_managed_account_profile(state, &account_id).await?;
            }
        }
        Ok(())
    }
    .await;
    restart_codex_after_failed_change(stopped, result, launch_codex_with_profile)?;
    Ok(stopped)
}

pub(super) fn restart_codex_after_failed_change<T>(
    stopped: bool,
    result: Result<T, CommandError>,
    launch: impl FnOnce() -> Result<(), String>,
) -> Result<T, CommandError> {
    match result {
        Err(mut error) if stopped => {
            if let Err(launch_error) = launch() {
                error.message = format!(
                    "{}; failed to restart ChatGPT: {launch_error}",
                    error.message
                );
            }
            Err(error)
        }
        result => result,
    }
}

pub(super) fn restart_codex_after_restore<T>(
    stopped: bool,
    result: Result<T, CommandError>,
    launch: impl FnOnce() -> Result<(), String>,
) -> Result<T, CommandError> {
    match result {
        Ok(value) if stopped => launch().map(|()| value).map_err(|error| {
            LocalPoolError::new(
                ErrorCode::Io,
                format!("profile restored, but ChatGPT failed to restart: {error}"),
            )
            .into()
        }),
        result => restart_codex_after_failed_change(stopped, result, launch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::cell::RefCell;

    #[tokio::test]
    async fn launch_applies_the_catalog_before_starting_the_client() {
        let events = RefCell::new(Vec::new());
        launch_after_catalog_refresh(
            true,
            async {
                tokio::task::yield_now().await;
                events.borrow_mut().push("catalog");
                Ok(())
            },
            |result| {
                assert!(result.is_ok());
                events.borrow_mut().push("report");
            },
            || {
                events.borrow_mut().push("launch");
                Ok(())
            },
        )
        .await
        .unwrap();
        assert_eq!(*events.borrow(), ["catalog", "report", "launch"]);
    }

    #[tokio::test]
    async fn unavailable_catalog_reports_warning_and_still_launches_verified_profile() {
        let reported = Cell::new(false);
        let launched = Cell::new(false);
        launch_after_catalog_refresh(
            true,
            async {
                Err::<(), _>(LocalPoolError::new(
                    ErrorCode::GatewayUnavailable,
                    "synthetic failure",
                ))
            },
            |result| reported.set(result.is_err()),
            || {
                assert!(reported.get());
                launched.set(true);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(launched.get());
    }

    #[tokio::test]
    async fn freshly_attached_catalog_is_not_fetched_again_at_launch() {
        let fetched = Cell::new(false);
        let reported = Cell::new(false);
        let launched = Cell::new(false);
        launch_after_catalog_refresh(
            false,
            async {
                fetched.set(true);
                Ok(())
            },
            |_| reported.set(true),
            || {
                launched.set(true);
                Ok(())
            },
        )
        .await
        .unwrap();
        assert!(launched.get());
        assert!(!fetched.get());
        assert!(!reported.get());
    }

    #[test]
    fn failed_profile_change_restarts_a_previously_running_codex() {
        let launched = Cell::new(false);
        let error = restart_codex_after_failed_change::<()>(
            true,
            Err(LocalPoolError::new(ErrorCode::Conflict, "profile conflict").into()),
            || {
                launched.set(true);
                Ok(())
            },
        )
        .unwrap_err();

        assert!(launched.get());
        assert!(matches!(error.code, ErrorCode::Conflict));
    }

    #[test]
    fn successful_restore_restarts_a_previously_running_codex() {
        let launched = Cell::new(false);
        restart_codex_after_restore(true, Ok(()), || {
            launched.set(true);
            Ok(())
        })
        .unwrap();

        assert!(launched.get());
    }
}
