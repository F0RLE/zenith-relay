use super::*;

async fn scope(
    state: &Arc<AppState>,
    id: &str,
    kind: RefreshKind,
) -> Result<(SourceRefreshFence, String), String> {
    let _configuration = state.configuration_lock.lock().await;
    let (record, fence) = state.store.source_refresh_scope(id)?;
    register(
        state,
        &record,
        fence.clone(),
        kind,
        false,
        is_active(&record, &active_runtime_members(state)?),
    )?;
    Ok((fence, record.base_url))
}

pub(crate) async fn request_models(
    state: &Arc<AppState>,
    id: &str,
) -> Result<SourceRecord, String> {
    let (fence, _) = scope(state, id, RefreshKind::Models).await?;
    let result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Models)
        .await
        .map_err(|_| "source refresh could not complete".to_string())?;
    let _configuration = state.configuration_lock.lock().await;
    let (_, current) = state.store.source_refresh_scope(id)?;
    if current != fence {
        return Err("source changed during refresh".into());
    }
    match result.as_ref().clone()? {
        RefreshRead::SourceModels(source) => Ok(*source),
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
        let (record, current) = state.store.source_refresh_scope(id)?;
        if current != fence || record.base_url != base_url {
            return Err("source changed during refresh".into());
        }
        if let Some(stats) = cached_stats(state, &fence, &base_url) {
            return Ok(stats);
        }
    }
    let result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Balance)
        .await
        .map_err(|_| "source refresh could not complete".to_string())?;
    let _configuration = state.configuration_lock.lock().await;
    let (record, current) = state.store.source_refresh_scope(id)?;
    if current != fence || record.base_url != base_url {
        return Err("source changed during refresh".into());
    }
    match result.as_ref().clone()? {
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
    let (read, freshness) = state
        .refresh
        .cached_observation(&fence.identity(), RefreshKind::Balance)?;
    match read.as_ref() {
        Ok(RefreshRead::SourceStats(observation)) => observation.snapshot(base_url, freshness),
        _ => None,
    }
}

pub(super) async fn execute(
    state: &Arc<AppState>,
    fence: &SourceRefreshFence,
    kind: RefreshKind,
    manual: bool,
) -> RefreshReadResult {
    let (before, source) = {
        let _configuration = state.configuration_lock.lock().await;
        let (before, current) = state.store.source_refresh_scope(&fence.source_id)?;
        if current != *fence || (!manual && !before.enabled) {
            return Err("source changed during refresh".into());
        }
        let gateway_base_url = format!(
            "{}/v1",
            state.config.public_base_url.as_str().trim_end_matches('/')
        );
        if source_points_to_gateway(&before.base_url, &gateway_base_url) {
            return Err(error_codes::SOURCE_SELF_ROUTE.into());
        }
        let api_key = state
            .vault
            .load(&before.secret_ref)?
            .ok_or_else(|| error_codes::SOURCE_SECRET_MISSING.to_string())?;
        let source = ProviderSource {
            id: before.id.clone(),
            name: before.name.clone(),
            base_url: before.base_url.clone(),
            api_key,
            wire_api: before.wire_api,
            models: before.models.clone(),
        };
        (before, source)
    };
    state.refresh.set_active(
        &fence.identity(),
        is_active(&before, &active_runtime_members(state)?),
    );
    match kind {
        RefreshKind::Models => {
            let read = zenith_relay_core::read_source_models_with_scope(
                &source,
                &before.protocol_bindings,
                &before.protocol_config,
                source_http_scope(state, fence),
            )
            .await;
            if let Some(delay) = read.retry_after_ms {
                state
                    .refresh
                    .respect_retry_after(&fence.identity(), kind, delay);
            }
            let _configuration = state.configuration_lock.lock().await;
            // A catalog can replace routes or resolve a new base URL. Do not
            // persist it while a previous runtime build can still publish an
            // old executor, or while pending leases can reach final dispatch.
            let build = state.lock_runtime_rebuild().await;
            validate(state, fence, &source)?;
            let previous = state.store.source_refresh_scope(&fence.source_id)?.0;
            let mut preview = previous.clone();
            if let Ok(discovery) = &read.value {
                apply_discovered_source(&mut preview, discovery)?;
            }
            let changed = source_catalog_changed(&previous, &preview);
            let _dispatch_fences = if changed {
                state
                    .runtime()?
                    .map(|runtime| runtime.fence_source_dispatch(&source.id))
            } else {
                None
            };
            let updated = state.store.apply_source_refresh(fence, |record| {
                match &read.value {
                    Ok(discovery) => {
                        apply_discovered_source(record, discovery)?;
                        record.last_error_code = None;
                    }
                    Err(_) => {
                        record.last_error_code = Some(
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
            if previous.base_url != updated.base_url {
                state
                    .refresh
                    .remove_kind(&fence.identity(), RefreshKind::Balance);
                state.store.notify_refresh_changed();
            }
            if changed {
                build
                    .rebuild_or_rollback(state, || {
                        state.store.apply_source_refresh(fence, |record| {
                            *record = previous.clone();
                            Ok(())
                        })?;
                        Ok(())
                    })
                    .await?;
            }
            Ok(RefreshRead::SourceModels(Box::new(updated)))
        }
        RefreshKind::Balance => {
            let read = zenith_relay_core::read_source_provider_stats_with_scope(
                &source.base_url,
                &source.api_key,
                source_http_scope(state, fence),
            )
            .await;
            if let Some(delay) = read.retry_after_ms {
                state
                    .refresh
                    .respect_retry_after(&fence.identity(), kind, delay);
            }
            let _configuration = state.configuration_lock.lock().await;
            validate(state, fence, &source)?;
            let cached = state.refresh.cached(&fence.identity(), kind);
            let previous = cached.as_ref().and_then(|read| match read.as_ref() {
                Ok(RefreshRead::SourceStats(observation)) => observation.current(&source.base_url),
                _ => None,
            });
            let value = read.value?.observed(previous, now_ms());
            Ok(RefreshRead::SourceStats(SourceStatsObservation::new(
                source.base_url,
                value,
            )))
        }
        _ => Err("unsupported source refresh kind".into()),
    }
}

fn apply_discovered_source(
    record: &mut SourceRecord,
    discovery: &SourceDiscovery,
) -> Result<(), String> {
    discovery.apply_catalog(
        &mut record.base_url,
        &mut record.models,
        &mut record.protocol_bindings,
        &mut record.protocol_config,
        &mut record.detected_model_prices,
    );
    record.effective_protocol_bindings().map(drop)
}

fn source_http_scope(
    state: &Arc<AppState>,
    fence: &SourceRefreshFence,
) -> zenith_relay_core::scheduler::refresh::http::ManagementHttpScope {
    let state = state.clone();
    let fence = fence.clone();
    zenith_relay_core::scheduler::refresh::http::ManagementHttpScope::checked(move || {
        state
            .store
            .source_refresh_scope(&fence.source_id)
            .is_ok_and(|(_, current)| current == fence)
    })
}

fn validate(
    state: &AppState,
    fence: &SourceRefreshFence,
    checked: &ProviderSource,
) -> Result<(), String> {
    let (record, current) = state.store.source_refresh_scope(&fence.source_id)?;
    if current != *fence
        || record.base_url != checked.base_url
        || state.vault.load(&record.secret_ref)?.as_deref() != Some(checked.api_key.as_str())
    {
        return Err("source changed during refresh".into());
    }
    Ok(())
}
