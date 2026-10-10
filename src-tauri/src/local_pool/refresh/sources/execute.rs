use super::{DesktopState, RefreshRead, RefreshReadResult};
use crate::local_pool::{
    commands::{
        connections::ensure_not_gateway_self_source, core_error, fence_runtime_candidates,
        profiles::refresh_active_client_catalogs, record_catalog_refresh_result,
        sync_records_or_rollback,
    },
    error::{ErrorCode, LocalPoolError, Result},
    store::{secret_store, SourceRefreshFence},
};
use zenith_relay_core::{
    scheduler::refresh::{RefreshKind, SourceStatsObservation},
    source_catalog_changed, ProviderSource,
};

pub(super) async fn execute(
    state: &DesktopState,
    fence: &SourceRefreshFence,
    kind: RefreshKind,
    manual: bool,
) -> RefreshReadResult {
    let (before, provider_source) = {
        let _mutation = state.setup_guard().await;
        let store = state.store()?;
        store.ensure_source_refresh_current(fence)?;
        let before = store
            .source(&fence.source_id)
            .cloned()
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
        if !manual
            && (!before.enabled
                || !state.background_session_active()
                || (kind == RefreshKind::Models
                    && before.last_test_status.as_deref() == Some("manual")))
        {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "source monitoring changed",
            ));
        }
        drop(store);
        ensure_not_gateway_self_source(state, &before.base_url)?;
        let api_key = secret_store::load(&before.secret_ref)?
            .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source secret is missing"))?;
        let provider_source = ProviderSource {
            id: before.id.clone(),
            name: before.name.clone(),
            base_url: before.base_url.clone(),
            api_key,
            wire_api: before.wire_api,
            models: before.models.clone(),
        };
        (before, provider_source)
    };
    update_activity(state, fence).await?;
    match kind {
        RefreshKind::Models => {
            let source_read = zenith_relay_core::read_source_models_with_scope(
                &provider_source,
                &before.protocol_bindings,
                &before.protocol_config,
                source_http_scope(state, fence),
            )
            .await;
            if let Some(delay) = source_read.retry_after_ms {
                state
                    .refresh
                    .respect_retry_after(&fence.identity(), kind, delay);
            }
            let _mutation = state.setup_guard().await;
            validate(state, fence, &provider_source)?;
            update_activity(state, fence).await?;
            let (old_sources, old_keys) = {
                let store = state.store()?;
                (store.sources().to_vec(), store.keys().to_vec())
            };
            // Discovery may replace endpoint/protocol/model evidence. A
            // pending lease on the old executor must not dispatch while the
            // durable observation is being applied and its runtime replaced.
            let runtime = state.gateway.runtime().await;
            let _dispatch_fences = if source_read.read_value.is_ok() {
                fence_runtime_candidates(
                    runtime.as_deref(),
                    &[],
                    std::slice::from_ref(&fence.source_id),
                )
            } else {
                Vec::new()
            };
            let mut changed = false;
            let updated = state
                .store()?
                .apply_source_refresh(fence, |source_record| {
                    match source_read.read_value {
                        Ok(discovery) => {
                            let source_before_refresh = source_record.clone();
                            discovery.apply_catalog(
                                &mut source_record.base_url,
                                &mut source_record.models,
                                &mut source_record.protocol_bindings,
                                &mut source_record.protocol_config,
                                &mut source_record.detected_model_prices,
                            );
                            changed = source_catalog_changed(&source_before_refresh, source_record);
                            source_record.last_test_status = Some("ok".into());
                            source_record.last_error = None;
                            source_record.normalize();
                            source_record
                                .validate_protocol_bindings()
                                .map_err(LocalPoolError::invalid_state)?;
                        }
                        Err(error) => {
                            source_record.last_test_status = Some("error".into());
                            source_record.last_error = Some(core_error(error).message);
                        }
                    }
                    source_record.last_test_at = Some(chrono::Utc::now().to_rfc3339());
                    Ok(())
                })?;
            if before.base_url != updated.base_url {
                state
                    .refresh
                    .remove_kind(&fence.identity(), RefreshKind::Balance);
                state.store()?.notify_refresh_changed();
            }
            if changed {
                sync_records_or_rollback(state, old_sources, old_keys).await?;
            }
            drop(_dispatch_fences);
            if changed || manual {
                let catalog_result = refresh_active_client_catalogs(state).await;
                record_catalog_refresh_result(state, &catalog_result);
            }
            Ok(RefreshRead::SourceModels(Box::new(updated)))
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
            let _mutation = state.setup_guard().await;
            validate(state, fence, &provider_source)?;
            update_activity(state, fence).await?;
            let provider_stats = stats_read
                .read_value
                .map_err(|message| LocalPoolError::new(ErrorCode::GatewayUnavailable, message))?;
            let cached = state.refresh.cached(&fence.identity(), kind);
            let previous_source_stats = cached.as_ref().and_then(|read| match read.as_ref() {
                Ok(RefreshRead::SourceStats(observation)) => {
                    observation.current(&provider_source.base_url)
                }
                _ => None,
            });
            Ok(RefreshRead::SourceStats(SourceStatsObservation::new(
                provider_source.base_url,
                provider_stats.observed(previous_source_stats, super::super::current_time_ms()),
            )))
        }
        _ => Err(LocalPoolError::invalid_state(
            "unsupported source refresh kind",
        )),
    }
}

fn source_http_scope(
    state: &DesktopState,
    fence: &SourceRefreshFence,
) -> zenith_relay_core::scheduler::refresh::http::ManagementHttpScope {
    let desktop_state = state.clone();
    let refresh_fence = fence.clone();
    zenith_relay_core::scheduler::refresh::http::ManagementHttpScope::checked(move || {
        desktop_state
            .store()
            .is_ok_and(|store| store.ensure_source_refresh_current(&refresh_fence).is_ok())
    })
}

async fn update_activity(state: &DesktopState, fence: &SourceRefreshFence) -> Result<()> {
    let activity = super::super::active_members(state).await;
    let store = state.store()?;
    store.ensure_source_refresh_current(fence)?;
    let (source_record, _) = store.source_refresh_scope(&fence.source_id)?;
    state.refresh.set_active(
        &fence.identity(),
        super::is_active(&source_record, &activity),
    );
    Ok(())
}
fn validate(
    state: &DesktopState,
    fence: &SourceRefreshFence,
    checked: &ProviderSource,
) -> Result<()> {
    let store = state.store()?;
    store.ensure_source_refresh_current(fence)?;
    let stored_source = store
        .source(&fence.source_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    if stored_source.base_url != checked.base_url
        || secret_store::load(&stored_source.secret_ref)?.as_deref()
            != Some(checked.api_key.as_str())
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source changed during refresh",
        ));
    }
    Ok(())
}
