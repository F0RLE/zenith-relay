//! API-source model/balance jobs share account traffic limits and lifecycle.
#[cfg(test)]
mod tests;

use super::{wait_error, DesktopState, RefreshRead, RefreshReadResult};
use crate::local_pool::{
    commands::{
        connections::ensure_not_gateway_self_source, core_error, fence_runtime_candidates,
        profiles::refresh_active_client_catalogs, record_catalog_refresh_result,
        sync_records_or_rollback,
    },
    error::{ErrorCode, LocalPoolError, Result},
    models::ProviderSourceRecord,
    store::{secret_store, LocalPoolStore, SourceRefreshFence},
};
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
};
use zenith_relay_core::{
    scheduler::refresh::{
        service::{RefreshRegistration, RefreshResult},
        source_stats_outcome, RefreshIdentity, RefreshKind, RefreshOutcome, SourceStatsObservation,
    },
    ProviderSource, SourceProviderStats,
};

pub(super) fn reconcile(
    state: &DesktopState,
    store: &LocalPoolStore,
    activity: &BTreeSet<String>,
    current: &mut BTreeSet<RefreshIdentity>,
) -> Result<()> {
    for source in store.sources() {
        let (_, fence) = store.source_refresh_scope(&source.id)?;
        current.insert(fence.identity());
        for kind in [RefreshKind::Models, RefreshKind::Balance] {
            register(state, source, fence.clone(), kind, true, activity)?;
        }
    }
    Ok(())
}

fn register(
    state: &DesktopState,
    source: &ProviderSourceRecord,
    fence: SourceRefreshFence,
    kind: RefreshKind,
    due_now: bool,
    activity: &BTreeSet<String>,
) -> Result<()> {
    let active = is_active(source, activity);
    let origin = url::Url::parse(source.base_url.trim())
        .map_err(|_| LocalPoolError::invalid_state("invalid source address"))?
        .origin()
        .ascii_serialization();
    let automatic = source.enabled
        && state.refresh_started.load(Ordering::Acquire)
        && state.background_session_active()
        && (kind != RefreshKind::Models || source.last_test_status.as_deref() != Some("manual"));
    let weak = Arc::downgrade(&state.owner);
    state
        .refresh
        .register(
            RefreshRegistration {
                identity: fence.identity(),
                kind,
                origin,
                active,
                automatic,
                due_now,
            },
            move |job| {
                let (weak, fence) = (weak.clone(), fence.clone());
                Box::pin(async move {
                    let Some(owner) = weak.upgrade() else {
                        return RefreshResult {
                            value: Err(LocalPoolError::invalid_state("refresh owner stopped")),
                            outcome: RefreshOutcome::NoProgress,
                        };
                    };
                    let state = DesktopState { owner };
                    let value = execute(&state, &fence, job.kind, job.manual).await;
                    let outcome = match &value {
                        Ok(RefreshRead::SourceModels(source))
                            if source.last_test_status.as_deref() == Some("ok") =>
                        {
                            RefreshOutcome::Success
                        }
                        Ok(RefreshRead::SourceStats(stats)) => {
                            source_stats_outcome(&stats.stats, state.refresh.now_ms())
                        }
                        _ => RefreshOutcome::FailedRetryAt(
                            state.refresh.now_ms().saturating_add(60_000),
                        ),
                    };
                    RefreshResult { value, outcome }
                })
            },
        )
        .map_err(wait_error)
}

fn is_active(source: &ProviderSourceRecord, activity: &BTreeSet<String>) -> bool {
    source
        .last_used_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
        .is_some_and(|at| super::recently_used(Some(at)))
        || activity.contains(&format!("source:{}", source.id))
}

async fn scope(
    state: &DesktopState,
    id: &str,
    kind: RefreshKind,
) -> Result<(SourceRefreshFence, String)> {
    let _mutation = state.setup_guard().await;
    let activity = super::active_members(state).await;
    let store = state.store()?;
    let (source, fence) = store.source_refresh_scope(id)?;
    register(state, &source, fence.clone(), kind, false, &activity)?;
    Ok((fence, source.base_url))
}

pub(crate) async fn request_models(
    state: &DesktopState,
    id: &str,
    replace_manual: bool,
) -> Result<ProviderSourceRecord> {
    let fence = {
        let _mutation = state.setup_guard().await;
        let activity = super::active_members(state).await;
        let store = state.store()?;
        let (source, fence) = store.source_refresh_scope(id)?;
        if !replace_manual && source.last_test_status.as_deref() == Some("manual") {
            return Ok(source);
        }
        register(
            state,
            &source,
            fence.clone(),
            RefreshKind::Models,
            false,
            &activity,
        )?;
        fence
    };
    let result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Models)
        .await
        .map_err(wait_error)?;
    let _mutation = state.setup_guard().await;
    state.store()?.ensure_source_refresh_current(&fence)?;
    match result.as_ref().clone()? {
        RefreshRead::SourceModels(source) => Ok(*source),
        _ => Err(LocalPoolError::invalid_state(
            "unexpected source models result",
        )),
    }
}

pub(crate) async fn request_stats(
    state: &DesktopState,
    id: &str,
    force: bool,
) -> Result<SourceProviderStats> {
    let (fence, base_url) = scope(state, id, RefreshKind::Balance).await?;
    if !force {
        let _mutation = state.setup_guard().await;
        ensure_stats_current(state, &fence, &base_url)?;
        if let Some(stats) = cached_stats(state, &fence, &base_url) {
            return Ok(stats);
        }
    }
    let result = state
        .refresh
        .request(&fence.identity(), RefreshKind::Balance)
        .await
        .map_err(wait_error)?;
    let _mutation = state.setup_guard().await;
    ensure_stats_current(state, &fence, &base_url)?;
    match result.as_ref().clone()? {
        RefreshRead::SourceStats(observation) => {
            observation.current(&base_url).cloned().ok_or_else(|| {
                LocalPoolError::new(ErrorCode::Conflict, "source changed during refresh")
            })
        }
        _ => Err(LocalPoolError::invalid_state(
            "unexpected source stats result",
        )),
    }
}

/// Read-only snapshot projection: never schedules provider HTTP.
pub(crate) fn cached_stats(
    state: &DesktopState,
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

fn ensure_stats_current(
    state: &DesktopState,
    fence: &SourceRefreshFence,
    base_url: &str,
) -> Result<()> {
    let store = state.store()?;
    store.ensure_source_refresh_current(fence)?;
    if store
        .source(&fence.source_id)
        .is_none_or(|record| record.base_url != base_url)
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source changed during refresh",
        ));
    }
    Ok(())
}

async fn execute(
    state: &DesktopState,
    fence: &SourceRefreshFence,
    kind: RefreshKind,
    manual: bool,
) -> RefreshReadResult {
    let (before, source) = {
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
    update_activity(state, fence).await?;
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
            let _mutation = state.setup_guard().await;
            validate(state, fence, &source)?;
            update_activity(state, fence).await?;
            let (old_sources, old_keys) = {
                let store = state.store()?;
                (store.sources().to_vec(), store.keys().to_vec())
            };
            // Discovery may replace endpoint/protocol/model evidence. A
            // pending lease on the old executor must not dispatch while the
            // durable observation is being applied and its runtime replaced.
            let runtime = state.gateway.runtime().await;
            let _dispatch_fences = if read.value.is_ok() {
                fence_runtime_candidates(
                    runtime.as_deref(),
                    &[],
                    std::slice::from_ref(&fence.source_id),
                )
            } else {
                Vec::new()
            };
            let mut changed = false;
            let updated = state.store()?.apply_source_refresh(fence, |record| {
                match read.value {
                    Ok(discovery) => {
                        let previous = record.clone();
                        if let Some(url) = discovery.resolved_base_url {
                            record.base_url = url;
                        }
                        record.models = discovery.models;
                        record.protocol_bindings = discovery.protocol_bindings;
                        record.protocol_config.merge_catalog(discovery.capabilities);
                        record.detected_model_prices = discovery.detected_model_prices;
                        changed = previous.models != record.models
                            || previous.base_url != record.base_url
                            || previous.protocol_bindings != record.protocol_bindings
                            || previous.protocol_config != record.protocol_config
                            || previous.detected_model_prices != record.detected_model_prices;
                        record.last_test_status = Some("ok".into());
                        record.last_error = None;
                        record.normalize();
                        record
                            .validate_protocol_bindings()
                            .map_err(LocalPoolError::invalid_state)?;
                    }
                    Err(error) => {
                        record.last_test_status = Some("error".into());
                        record.last_error = Some(core_error(error).message);
                    }
                }
                record.last_test_at = Some(chrono::Utc::now().to_rfc3339());
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
            let _mutation = state.setup_guard().await;
            validate(state, fence, &source)?;
            update_activity(state, fence).await?;
            let value = read
                .value
                .map_err(|message| LocalPoolError::new(ErrorCode::GatewayUnavailable, message))?;
            let cached = state.refresh.cached(&fence.identity(), kind);
            let previous = cached.as_ref().and_then(|read| match read.as_ref() {
                Ok(RefreshRead::SourceStats(observation)) => observation.current(&source.base_url),
                _ => None,
            });
            Ok(RefreshRead::SourceStats(SourceStatsObservation::new(
                source.base_url,
                value.observed(previous, super::current_time_ms()),
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
    let state = state.clone();
    let fence = fence.clone();
    zenith_relay_core::scheduler::refresh::http::ManagementHttpScope::checked(move || {
        state
            .store()
            .is_ok_and(|store| store.ensure_source_refresh_current(&fence).is_ok())
    })
}

async fn update_activity(state: &DesktopState, fence: &SourceRefreshFence) -> Result<()> {
    let activity = super::active_members(state).await;
    let store = state.store()?;
    store.ensure_source_refresh_current(fence)?;
    let (current, _) = store.source_refresh_scope(&fence.source_id)?;
    state
        .refresh
        .set_active(&fence.identity(), is_active(&current, &activity));
    Ok(())
}
fn validate(
    state: &DesktopState,
    fence: &SourceRefreshFence,
    checked: &ProviderSource,
) -> Result<()> {
    let store = state.store()?;
    store.ensure_source_refresh_current(fence)?;
    let record = store
        .source(&fence.source_id)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "source not found"))?;
    if record.base_url != checked.base_url
        || secret_store::load(&record.secret_ref)?.as_deref() != Some(checked.api_key.as_str())
    {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "source changed during refresh",
        ));
    }
    Ok(())
}
