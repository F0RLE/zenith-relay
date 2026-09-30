//! API-source catalog and balance reads use the same admission and revisions
//! as account jobs. Only the database observation owner may merge a late read.
use super::{active_runtime_members, AppState, RefreshRead, RefreshReadResult};
use crate::{
    state::{now_ms, SourceRecord},
    store::SourceRefreshFence,
};
use std::{collections::BTreeSet, sync::Arc};
use zenith_relay_core::{
    error_codes,
    scheduler::{
        refresh::{
            service::{RefreshRegistration, RefreshResult},
            source_stats_outcome, RefreshIdentity, RefreshKind, RefreshOutcome,
            SourceStatsObservation,
        },
        source_member_key,
    },
    source_catalog_changed, source_points_to_gateway, ProviderSource, SourceDiscovery,
    SourceProviderStats,
};

pub(super) fn reconcile(
    state: &Arc<AppState>,
    activity: &BTreeSet<String>,
    current: &mut BTreeSet<RefreshIdentity>,
) -> Result<(), String> {
    for (record, fence) in state.store.source_refresh_scopes()? {
        current.insert(fence.identity());
        for kind in [RefreshKind::Models, RefreshKind::Balance] {
            register(
                state,
                &record,
                fence.clone(),
                kind,
                true,
                is_active(&record, activity),
            )?;
        }
    }
    Ok(())
}

fn is_active(record: &SourceRecord, activity: &BTreeSet<String>) -> bool {
    activity.contains(&source_member_key(&record.id))
}

fn register(
    state: &Arc<AppState>,
    record: &SourceRecord,
    fence: SourceRefreshFence,
    kind: RefreshKind,
    due_now: bool,
    active: bool,
) -> Result<(), String> {
    let origin = url::Url::parse(record.base_url.trim())
        .map_err(|_| "invalid source address".to_string())?
        .origin()
        .ascii_serialization();
    let automatic = record.enabled;
    let weak = Arc::downgrade(state);
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
                    let Some(state) = weak.upgrade() else {
                        return RefreshResult {
                            value: Err("refresh owner stopped".into()),
                            outcome: RefreshOutcome::NoProgress,
                        };
                    };
                    let value = read::execute(&state, &fence, job.kind, job.manual).await;
                    let outcome = match &value {
                        Ok(RefreshRead::SourceModels(source))
                            if source.last_error_code.is_none() =>
                        {
                            RefreshOutcome::Success
                        }
                        Ok(RefreshRead::SourceStats(stats)) => {
                            source_stats_outcome(&stats.stats, state.refresh.now_ms())
                        }
                        _ => RefreshOutcome::retry_after(state.refresh.now_ms(), None),
                    };
                    RefreshResult { value, outcome }
                })
            },
        )
        .map_err(|_| "source refresh could not be scheduled".to_string())
}

mod read;

pub(crate) use read::{cached_stats, request_models, request_stats};

#[cfg(test)]
mod tests;
