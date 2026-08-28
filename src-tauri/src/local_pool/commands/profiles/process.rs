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
