use super::{error::CommandError, snapshot};
use tauri::AppHandle;

#[tauri::command]
pub fn get_local_pool_state(
    app: AppHandle,
) -> Result<super::models::LocalPoolSnapshot, CommandError> {
    snapshot(&app).map_err(Into::into)
}
