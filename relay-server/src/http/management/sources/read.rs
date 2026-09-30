use super::*;

pub async fn list_sources(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<SourceSummary>>, ManagementError> {
    Ok(Json(state.snapshot().map_err(store_error)?.sources))
}

pub async fn test_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<SourceSummary>, ManagementError> {
    let record = find_source(&state, &id)?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    let record = crate::jobs::request_source_models(&state, &id)
        .await
        .map_err(|error| {
            if error.contains("changed during refresh") {
                stale_probe_error()
            } else if error == error_codes::SOURCE_SECRET_MISSING {
                ManagementError::not_found(
                    error_codes::SOURCE_SECRET_MISSING,
                    "source secret missing",
                )
            } else {
                ManagementError::new(
                    StatusCode::BAD_GATEWAY,
                    error_codes::SOURCE_TEST_FAILED,
                    "source catalog is unavailable",
                    "source",
                    true,
                )
            }
        })?;
    Ok(Json(source_summary(&state, &record)?))
}

fn stale_probe_error() -> ManagementError {
    ManagementError::new(
        StatusCode::CONFLICT,
        error_codes::SOURCE_PROBE_STALE,
        "source changed during the check; refresh and try again",
        "source",
        false,
    )
}

pub async fn probe_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(input): Json<zenith_relay_core::SourceProbeInput>,
) -> Result<Json<zenith_relay_core::SourceProbeResult>, ManagementError> {
    let record = find_source(&state, &id)?;
    if record.protocol_config.revision != input.expected_revision {
        return Err(stale_probe_error());
    }
    ensure_not_server_self_source(&state, &record.base_url)?;
    let api_key = state
        .vault
        .load(&record.secret_ref)
        .map_err(vault_error)?
        .ok_or_else(|| {
            ManagementError::not_found(error_codes::SOURCE_SECRET_MISSING, "source secret missing")
        })?;
    let source = ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.clone(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    };
    let (_, refresh_fence) = state.store.source_refresh_scope(&id).map_err(store_error)?;
    let refresh_state = state.clone();
    let refresh_fence_for_http = refresh_fence.clone();
    let http_scope =
        zenith_relay_core::scheduler::refresh::http::ManagementHttpScope::checked(move || {
            refresh_state
                .store
                .source_refresh_scope(&refresh_fence_for_http.source_id)
                .is_ok_and(|(_, current)| current == refresh_fence_for_http)
        });
    let result = zenith_relay_core::probe_source_generation_with_scope(&source, &input, http_scope)
        .await
        .map_err(source_discovery_error)?;
    let _configuration = state.configuration_lock.lock().await;
    let mut current = find_source(&state, &id)?;
    let previous = current.clone();
    let (_, current_fence) = state.store.source_refresh_scope(&id).map_err(store_error)?;
    // Comparing visible fields alone accepts a delete/re-add with the same
    // source ID and configuration. The durable incarnation must still match.
    if current_fence != refresh_fence
        || current.base_url != record.base_url
        || current.models != record.models
        || current.protocol_config != record.protocol_config
        || state
            .vault
            .load(&current.secret_ref)
            .map_err(vault_error)?
            .as_deref()
            != Some(api_key.as_str())
        || !current
            .protocol_config
            .apply_probe(input.expected_revision, result.capability.clone())
    {
        return Err(stale_probe_error());
    }
    normalize_record_protocol_bindings(&mut current)?;
    state.store.save_source(&current).map_err(store_error)?;
    state
        .rebuild_runtime_or_rollback(|| state.store.save_source(&previous))
        .await
        .map_err(runtime_error)?;
    Ok(Json(result))
}

pub async fn source_stats(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<SourceStatsQuery>,
) -> Result<Json<zenith_relay_core::SourceProviderStats>, ManagementError> {
    let record = find_source(&state, &id)?;
    ensure_not_server_self_source(&state, &record.base_url)?;
    crate::jobs::request_source_stats(&state, &id, query.force)
        .await
        .map(Json)
        .map_err(|error| {
            if error == error_codes::SOURCE_SECRET_MISSING {
                return ManagementError::not_found(
                    error_codes::SOURCE_SECRET_MISSING,
                    "source secret is missing",
                );
            }
            if error.contains("changed during refresh") || error == "source not found" {
                return stale_probe_error();
            }
            ManagementError::new(
                StatusCode::BAD_GATEWAY,
                error_codes::SOURCE_STATS_UNAVAILABLE,
                "source stats are unavailable",
                "source",
                true,
            )
        })
}

pub(super) async fn discover_models(
    record: &SourceRecord,
    api_key: &str,
) -> Result<SourceDiscovery, ManagementError> {
    let source = ProviderSource {
        id: record.id.clone(),
        name: record.name.clone(),
        base_url: record.base_url.clone(),
        api_key: api_key.to_string(),
        wire_api: record.wire_api,
        models: record.models.clone(),
    };
    let discovery = discover_source_with_protocol_config(
        &source,
        &record.protocol_bindings,
        &record.protocol_config,
    )
    .await
    .map_err(source_discovery_error)?;
    Ok(discovery)
}

pub(super) fn source_discovery_error(error: zenith_relay_core::Error) -> ManagementError {
    let (status, retryable) = match &error {
        zenith_relay_core::Error::Validation(_) | zenith_relay_core::Error::UnsupportedWireApi => {
            (StatusCode::BAD_REQUEST, false)
        }
        zenith_relay_core::Error::UpstreamStatus(status) => (
            StatusCode::BAD_GATEWAY,
            *status == 408 || *status == 429 || *status >= 500,
        ),
        zenith_relay_core::Error::ManagementHttpUnavailable
        | zenith_relay_core::Error::Upstream(_)
        | zenith_relay_core::Error::UpstreamBodyTooLarge
        | zenith_relay_core::Error::InvalidUpstreamResponse(_) => (StatusCode::BAD_GATEWAY, true),
    };
    ManagementError::new(
        status,
        error_codes::SOURCE_TEST_FAILED,
        error.to_string(),
        "upstream",
        retryable,
    )
}
