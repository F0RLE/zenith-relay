use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    profiles::repair,
    state::DesktopState,
};

#[derive(Clone, Copy)]
pub(crate) enum CodexHistoryProvider {
    ChatGpt,
    LocalGateway,
    ReadyApi,
}

/// Decide whether the history repair pass should run before a profile change.
///
/// The credential kind describes the currently attached profile, but it does
/// not describe every rollout and SQLite row already present in the Codex
/// home. In particular, a profile can already be attached to the local Relay
/// while a chat imported from another client still carries
/// `codex_local_access`. Skipping the repair in that state leaves the saved
/// chat bound to the wrong adapter and the next request never reaches Relay.
///
/// The repair pass is idempotent and creates no backup when all rows already
/// match, so run it for every profile change instead of using the credential
/// kind as a proxy for history state. This also covers the first Relay attach,
/// where no Relay-owned profile backup exists yet.
pub(crate) fn history_provider_changed(
    _state: &DesktopState,
    _profile_dir: &std::path::Path,
    _target: CodexHistoryProvider,
) -> Result<bool, String> {
    Ok(true)
}

pub(crate) fn synchronize_codex_history(
    state: &DesktopState,
    profile_dir: &std::path::Path,
    provider: CodexHistoryProvider,
) -> Result<Option<String>, String> {
    let provider = match provider {
        CodexHistoryProvider::ChatGpt => repair::TargetProvider::Openai,
        CodexHistoryProvider::LocalGateway => repair::TargetProvider::ZenithRelayLocal,
        CodexHistoryProvider::ReadyApi => repair::TargetProvider::CodexLocalAccess,
    };
    repair::synchronize(
        &state.transient_root(),
        &state.history_repair_backup_root(),
        profile_dir,
        provider,
    )
    .map(|result| result.map(|result| result.backup_id))
}

pub(crate) fn rollback_codex_history(state: &DesktopState, backup_id: &str) -> Result<(), String> {
    repair::rollback(&state.history_repair_backup_root(), backup_id)?;
    repair::discard(&state.history_repair_backup_root(), backup_id)
}

pub(crate) fn discard_codex_history_backup(state: &DesktopState, backup_id: Option<&str>) {
    if let Some(backup_id) = backup_id {
        let _ = repair::discard(&state.history_repair_backup_root(), backup_id);
    }
}

pub(super) fn synchronize_history_for_command(
    state: &DesktopState,
    profile_dir: &std::path::Path,
    provider: CodexHistoryProvider,
) -> Result<Option<String>, CommandError> {
    synchronize_codex_history(state, profile_dir, provider)
        .map_err(|message| LocalPoolError::new(ErrorCode::RecoveryRequired, message).into())
}

pub(super) fn rollback_history_on_error<T>(
    state: &DesktopState,
    backup_id: Option<&str>,
    result: Result<T, CommandError>,
) -> Result<T, CommandError> {
    match result {
        Ok(value) => {
            discard_codex_history_backup(state, backup_id);
            Ok(value)
        }
        Err(mut error) => {
            if let Some(backup_id) = backup_id {
                if let Err(rollback) = rollback_codex_history(state, backup_id) {
                    error.message = format!(
                        "{}; automatic history rollback failed: {rollback}",
                        error.message
                    );
                }
            }
            Err(error)
        }
    }
}
