use super::*;

#[tauri::command]
pub async fn test_local_source(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<ProviderSourceRecord> {
    crate::local_pool::refresh::sources::request_models(&state, &source_id, true)
        .await
        .map_err(Into::into)
}

/// Refresh the source model catalog from the management view without
/// replacing a manual model list. The UI pairs this operation with the
/// provider-statistics request so both values are refreshed by one action.
#[tauri::command]
pub async fn refresh_local_source_data(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<ProviderSourceRecord> {
    crate::local_pool::refresh::sources::request_models(&state, &source_id, false)
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn get_local_source_stats(
    source_id: String,
    force: Option<bool>,
    state: State<'_, DesktopState>,
) -> CommandResult<SourceProviderStats> {
    crate::local_pool::refresh::sources::request_stats(&state, &source_id, force.unwrap_or(false))
        .await
        .map_err(Into::into)
}
