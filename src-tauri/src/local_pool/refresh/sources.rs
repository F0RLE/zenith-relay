//! API-source model/balance jobs share account traffic limits and lifecycle.
mod execute;

#[cfg(test)]
mod tests;

use super::{wait_error, DesktopState, RefreshRead, RefreshReadResult};
use crate::local_pool::{
    error::{ErrorCode, LocalPoolError, Result},
    models::ProviderSourceRecord,
    store::{LocalPoolStore, SourceRefreshFence},
};
use std::{
    collections::BTreeSet,
    sync::{atomic::Ordering, Arc},
};
use zenith_relay_core::{
    scheduler::{
        refresh::{
            service::{RefreshRegistration, RefreshResult},
            source_stats_outcome, RefreshIdentity, RefreshKind, RefreshOutcome,
            SourceStatsObservation,
        },
        source_member_key,
    },
    SourceProviderStats,
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
                    let value = execute::execute(&state, &fence, job.kind, job.manual).await;
                    let outcome = match &value {
                        Ok(RefreshRead::SourceModels(source))
                            if source.last_test_status.as_deref() == Some("ok") =>
                        {
                            RefreshOutcome::Success
                        }
                        Ok(RefreshRead::SourceStats(source_stats)) => {
                            source_stats_outcome(&source_stats.stats, state.refresh.now_ms())
                        }
                        _ => RefreshOutcome::retry_after(state.refresh.now_ms(), None),
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
        .and_then(zenith_relay_core::unix_time_ms_from_rfc3339)
        .is_some_and(|at| super::recently_used(Some(at)))
        || activity.contains(&source_member_key(&source.id))
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
    SourceStatsObservation::read_cached(
        state
            .refresh
            .cached_observation(&fence.identity(), RefreshKind::Balance),
        base_url,
    )
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
