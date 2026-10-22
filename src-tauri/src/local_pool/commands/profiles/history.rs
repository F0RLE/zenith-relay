use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    profiles::repair,
    state::DesktopState,
};

#[derive(Clone, Copy)]
pub(super) enum CodexHistoryProvider {
    ChatGpt,
    LocalGateway,
}

/// Repair is idempotent: check every profile switch, not just credential changes.
fn synchronize_codex_history(
    state: &DesktopState,
    profile_dir: &std::path::Path,
    provider: CodexHistoryProvider,
) -> Result<Option<String>, String> {
    let provider = match provider {
        CodexHistoryProvider::ChatGpt => repair::TargetProvider::Openai,
        CodexHistoryProvider::LocalGateway => repair::TargetProvider::ZenithRelayLocal,
    };
    repair::synchronize(
        &state.transient_root(),
        &state.history_repair_backup_root(),
        profile_dir,
        provider,
    )
    .map(|synchronization_result| {
        synchronization_result.map(|repair_outcome| repair_outcome.backup_id)
    })
}

fn rollback_codex_history(state: &DesktopState, backup_id: &str) -> Result<(), String> {
    repair::rollback(&state.history_repair_backup_root(), backup_id)?;
    repair::discard(&state.history_repair_backup_root(), backup_id)
}

fn discard_codex_history_backup(state: &DesktopState, backup_id: Option<&str>) {
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
    operation_result: Result<T, CommandError>,
) -> Result<T, CommandError> {
    match operation_result {
        Ok(operation_value) => {
            discard_codex_history_backup(state, backup_id);
            Ok(operation_value)
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
