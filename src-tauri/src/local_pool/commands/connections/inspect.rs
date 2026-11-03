use super::*;

#[tauri::command]
pub async fn check_local_source_connection(
    source_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<zenith_relay_core::SourceConnectionDiagnostics> {
    let (source, bindings) = {
        let _mutation = state.setup_guard().await;
        let record = state
            .store()?
            .source(source_id.trim())
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
        ensure_not_gateway_self_source(&state, &record.base_url)?;
        let api_key = secret_store::load(&record.secret_ref)?
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
        let proxy_url = source_proxy_url(&record)?;
        let source = ProviderSource {
            id: record.id,
            name: record.name,
            base_url: record.base_url,
            api_key,
            wire_api: record.wire_api,
            models: record.models,
            proxy_url,
        };
        (source, record.protocol_bindings)
    };
    Ok(
        zenith_relay_core::check_source_connection(&source, &bindings)
            .await
            .map_err(core_error)?,
    )
}

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
