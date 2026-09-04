use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    profiles::{codex, repair},
    state::DesktopState,
};

#[derive(Clone, Copy)]
pub(crate) enum CodexHistoryProvider {
    ChatGpt,
    LocalGateway,
    ReadyApi,
}

/// A provider metadata rewrite is only needed when the profile crosses an
/// adapter boundary. Switching two OAuth accounts or two Relay keys keeps the
/// same history provider and must not rescan the whole Codex home.
pub(crate) fn history_provider_changed(
    state: &DesktopState,
    profile_dir: &std::path::Path,
    target: CodexHistoryProvider,
) -> Result<bool, String> {
    let current = codex::credential_kind(profile_dir, &state.profile_backup_root())
        .map_err(|error| error.message)?;
    let same = match target {
        CodexHistoryProvider::ChatGpt => {
            current == Some(codex::ProfileCredentialKind::OAuthAccount)
        }
        CodexHistoryProvider::LocalGateway => {
            current == Some(codex::ProfileCredentialKind::LocalGateway)
        }
        CodexHistoryProvider::ReadyApi => current == Some(codex::ProfileCredentialKind::ApiKey),
    };
    Ok(!same)
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
