use super::*;

async fn scope(
    state: &Arc<AppState>,
    id: &str,
    kind: RefreshKind,
) -> Result<(SourceRefreshFence, String), String> {
    let _configuration = state.configuration_lock.lock().await;
    let (source_record, fence) = state.store.source_refresh_scope(id)?;
    register(
        state,
        &source_record,
        fence.clone(),
        kind,
        false,
        is_active(&source_record, &active_runtime_members(state)?),
    )?;
    Ok((fence, source_record.base_url))
}

pub(crate) async fn request_models(
    state: &Arc<AppState>,
    id: &str,
) -> Result<SourceRecord, String> {
    let (fence, _) = scope(state, id, RefreshKind::Models).await?;
    let models_refresh_result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Models)
        .await
        .map_err(|_| "source refresh could not complete".to_string())?;
    let _configuration = state.configuration_lock.lock().await;
    let (_, stored_fence) = state.store.source_refresh_scope(id)?;
    if stored_fence != fence {
        return Err("source changed during refresh".into());
    }
    match models_refresh_result.as_ref().clone()? {
        RefreshRead::SourceModels(source_record) => Ok(*source_record),
        _ => Err("unexpected source model result".into()),
    }
}

pub(crate) async fn request_stats(
    state: &Arc<AppState>,
    id: &str,
    force: bool,
) -> Result<SourceProviderStats, String> {
    let (fence, base_url) = scope(state, id, RefreshKind::Balance).await?;
    if !force {
        let _configuration = state.configuration_lock.lock().await;
        let (source_record, stored_fence) = state.store.source_refresh_scope(id)?;
        if stored_fence != fence || source_record.base_url != base_url {
            return Err("source changed during refresh".into());
        }
        if let Some(stats) = cached_stats(state, &fence, &base_url) {
            return Ok(stats);
        }
    }
    let stats_refresh_result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Balance)
        .await
        .map_err(|_| "source refresh could not complete".to_string())?;
    let _configuration = state.configuration_lock.lock().await;
    let (source_record, stored_fence) = state.store.source_refresh_scope(id)?;
    if stored_fence != fence || source_record.base_url != base_url {
        return Err("source changed during refresh".into());
    }
    match stats_refresh_result.as_ref().clone()? {
        RefreshRead::SourceStats(observation) => observation
            .current(&base_url)
            .cloned()
            .ok_or_else(|| "source changed during refresh".into()),
        _ => Err("unexpected source stats result".into()),
    }
}

/// Read-only snapshot projection: never schedules provider HTTP.
pub(crate) fn cached_stats(
    state: &AppState,
    fence: &SourceRefreshFence,
    base_url: &str,
) -> Option<SourceProviderStats> {
    SourceStatsObservation::read_cached(
        state
            .refresh
            .cached_observation(&fence.identity(), RefreshKind::Balance),
        base_url,
    )
}

pub(super) async fn execute(
    state: &Arc<AppState>,
    fence: &SourceRefreshFence,
    kind: RefreshKind,
    manual: bool,
) -> RefreshReadResult {
    let (source_before_refresh, provider_source) = {
        let _configuration = state.configuration_lock.lock().await;
        let (source_record, stored_fence) = state.store.source_refresh_scope(&fence.source_id)?;
        if stored_fence != *fence || (!manual && !source_record.enabled) {
            return Err("source changed during refresh".into());
        }
        let gateway_base_url = format!(
            "{}/v1",
            state.config.public_base_url.as_str().trim_end_matches('/')
        );
        if source_points_to_gateway(&source_record.base_url, &gateway_base_url) {
            return Err(error_codes::SOURCE_SELF_ROUTE.into());
        }
        let api_key = state
            .vault
            .load(&source_record.secret_ref)?
            .ok_or_else(|| error_codes::SOURCE_SECRET_MISSING.to_string())?;
        let provider_source = ProviderSource {
            id: source_record.id.clone(),
            name: source_record.name.clone(),
            base_url: source_record.base_url.clone(),
            api_key,
            wire_api: source_record.wire_api,
            models: source_record.models.clone(),
        };
        (source_record, provider_source)
    };
    state.refresh.set_active(
        &fence.identity(),
        is_active(&source_before_refresh, &active_runtime_members(state)?),
    );
    match kind {
        RefreshKind::Models => {
            let source_read = zenith_relay_core::read_source_models_with_scope(
                &provider_source,
                &source_before_refresh.protocol_bindings,
                &source_before_refresh.protocol_config,
                source_http_scope(state, fence),
            )
            .await;
            if let Some(delay) = source_read.retry_after_ms {
                state
                    .refresh
                    .respect_retry_after(&fence.identity(), kind, delay);
            }
            let _configuration = state.configuration_lock.lock().await;
            // A catalog can replace routes or resolve a new base URL. Do not
            // persist it while a previous runtime build can still publish an
            // old executor, or while pending leases can reach final dispatch.
            let build = state.lock_runtime_rebuild().await;
            validate(state, fence, &provider_source)?;
            let previous_source_record = state.store.source_refresh_scope(&fence.source_id)?.0;
            let mut preview = previous_source_record.clone();
            if let Ok(discovery) = &source_read.read_value {
                apply_discovered_source(&mut preview, discovery)?;
            }
            let changed = source_catalog_changed(&previous_source_record, &preview);
            let _dispatch_fences = if changed {
                state
                    .runtime()?
                    .map(|runtime| runtime.fence_source_dispatch(&provider_source.id))
            } else {
                None
            };
            let refreshed_source_record =
                state.store.apply_source_refresh(fence, |source_record| {
                    match &source_read.read_value {
                        Ok(discovery) => {
                            apply_discovered_source(source_record, discovery)?;
                            source_record.last_error_code = None;
                        }
                        Err(_) => {
                            source_record.last_error_code = Some(
                                if manual {
                                    error_codes::SOURCE_TEST_FAILED
                                } else {
                                    error_codes::SOURCE_MODEL_DISCOVERY_FAILED
                                }
                                .into(),
                            )
                        }
                    }
                    Ok(())
                })?;
            if previous_source_record.base_url != refreshed_source_record.base_url {
                state
                    .refresh
                    .remove_kind(&fence.identity(), RefreshKind::Balance);
                state.store.notify_refresh_changed();
            }
            if changed {
                build
                    .rebuild_or_rollback(state, || {
                        state.store.apply_source_refresh(fence, |source_record| {
                            *source_record = previous_source_record.clone();
                            Ok(())
                        })?;
                        Ok(())
                    })
                    .await?;
            }
            Ok(RefreshRead::SourceModels(Box::new(refreshed_source_record)))
        }
        RefreshKind::Balance => {
            let stats_read = zenith_relay_core::read_source_provider_stats_with_scope(
                &provider_source.base_url,
                &provider_source.api_key,
                source_http_scope(state, fence),
            )
            .await;
            if let Some(delay) = stats_read.retry_after_ms {
                state
                    .refresh
                    .respect_retry_after(&fence.identity(), kind, delay);
            }
            let _configuration = state.configuration_lock.lock().await;
            validate(state, fence, &provider_source)?;
            let cached = state.refresh.cached(&fence.identity(), kind);
            let previous_stats = cached.as_ref().and_then(|read| match read.as_ref() {
                Ok(RefreshRead::SourceStats(observation)) => {
                    observation.current(&provider_source.base_url)
                }
                _ => None,
            });
            let provider_stats = stats_read.read_value?.observed(previous_stats, now_ms());
            Ok(RefreshRead::SourceStats(SourceStatsObservation::new(
                provider_source.base_url,
                provider_stats,
            )))
        }
        _ => Err("unsupported source refresh kind".into()),
    }
}

fn apply_discovered_source(
    source_record: &mut SourceRecord,
    discovery: &SourceDiscovery,
) -> Result<(), String> {
    discovery.apply_catalog(
        &mut source_record.base_url,
        &mut source_record.models,
        &mut source_record.protocol_bindings,
        &mut source_record.protocol_config,
        &mut source_record.detected_model_prices,
    );
    source_record.effective_protocol_bindings().map(drop)
}

fn source_http_scope(
    state: &Arc<AppState>,
    fence: &SourceRefreshFence,
) -> zenith_relay_core::scheduler::refresh::http::ManagementHttpScope {
    let app_state = state.clone();
    let refresh_fence = fence.clone();
    zenith_relay_core::scheduler::refresh::http::ManagementHttpScope::checked(move || {
        app_state
            .store
            .source_refresh_scope(&refresh_fence.source_id)
            .is_ok_and(|(_, stored_fence)| stored_fence == refresh_fence)
    })
}

fn validate(
    state: &AppState,
    fence: &SourceRefreshFence,
    checked: &ProviderSource,
) -> Result<(), String> {
    let (source_record, stored_fence) = state.store.source_refresh_scope(&fence.source_id)?;
    if stored_fence != *fence
        || source_record.base_url != checked.base_url
        || state.vault.load(&source_record.secret_ref)?.as_deref() != Some(checked.api_key.as_str())
    {
        return Err("source changed during refresh".into());
    }
    Ok(())
}
