use crate::local_pool::{error::CommandError, models::LocalPoolSnapshot, state::DesktopState};
use tauri::State;

#[tauri::command]
pub async fn get_local_pool_state(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    state.snapshot().await.map_err(Into::into)
}
