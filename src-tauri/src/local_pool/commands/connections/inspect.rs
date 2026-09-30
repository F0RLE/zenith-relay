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

#[tauri::command]
pub async fn probe_local_source(
    source_id: String,
    input: zenith_relay_core::SourceProbeInput,
    state: State<'_, DesktopState>,
) -> CommandResult<zenith_relay_core::SourceProbeResult> {
    let source = state
        .store()?
        .source(&source_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    if input.expected_revision != source.protocol_config.revision {
        return Err(LocalPoolError::new(
            ErrorCode::SourceProbeStale,
            "source configuration changed; refresh before checking",
        )
        .into());
    }
    let api_key = secret_store::load(&source.secret_ref)?
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
    ensure_not_gateway_self_source(&state, &source.base_url)?;
    let runtime_source = ProviderSource {
        id: source.id.clone(),
        name: source.name.clone(),
        base_url: source.base_url.clone(),
        api_key: api_key.clone(),
        wire_api: source.wire_api,
        models: source.models.clone(),
    };
    let (_, refresh_fence) = state.store()?.source_refresh_scope(&source_id)?;
    let refresh_state = state.inner().clone();
    let refresh_fence_for_http = refresh_fence.clone();
    let http_scope =
        zenith_relay_core::scheduler::refresh::http::ManagementHttpScope::checked(move || {
            refresh_state.store().is_ok_and(|store| {
                store
                    .ensure_source_refresh_current(&refresh_fence_for_http)
                    .is_ok()
            })
        });
    let result =
        zenith_relay_core::probe_source_generation_with_scope(&runtime_source, &input, http_scope)
            .await
            .map_err(core_error)?;
    let _mutation = state.setup_guard().await;
    let (mut current, same_incarnation) = {
        let store = state.store()?;
        let current = store
            .source(&source_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
        (
            current,
            store.ensure_source_refresh_current(&refresh_fence).is_ok(),
        )
    };
    // Visible configuration can be identical after delete/re-add. The probe
    // still belongs to the prior durable source incarnation in that case.
    if !same_incarnation
        || !source_probe_matches(&source, &current)
        || secret_store::load(&current.secret_ref)?.as_deref() != Some(api_key.as_str())
        || !current
            .protocol_config
            .apply_probe(input.expected_revision, result.capability.clone())
    {
        return Err(LocalPoolError::new(
            ErrorCode::SourceProbeStale,
            "source changed during generation check; result discarded",
        )
        .into());
    }
    let (old_sources, old_keys) = current_records(&state)?;
    current
        .validate_protocol_bindings()
        .map_err(LocalPoolError::invalid_state)?;
    let runtime = state.gateway.runtime().await;
    let _dispatch_fences =
        fence_runtime_candidates(runtime.as_deref(), &[], std::slice::from_ref(&source_id));
    state.store()?.upsert_source(current)?;
    sync_records_or_rollback(&state, old_sources, old_keys).await?;
    Ok(result)
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
