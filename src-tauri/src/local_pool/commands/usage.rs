use crate::local_pool::{error::CommandError, state::DesktopState, store::telemetry_db::UsageLog};
use tauri::State;

#[tauri::command]
pub fn get_local_usage(
    limit: Option<u16>,
    state: State<'_, DesktopState>,
) -> Result<Vec<UsageLog>, CommandError> {
    state
        .telemetry
        .list(limit.unwrap_or(100))
        .map_err(Into::into)
}
