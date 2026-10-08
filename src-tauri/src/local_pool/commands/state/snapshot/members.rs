use std::collections::BTreeMap;

use crate::local_pool::error::LocalPoolError;
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::{DesktopState, SnapshotInputs};
use crate::local_pool::store::telemetry_db::{TelemetryDb, UsageEquivalents};
use zenith_relay_core::pricing::{PricingCatalog, PricingContext};
use zenith_relay_core::protocol::{AccountSummary, QuotaWindowUsage, SourceSummary};
use zenith_relay_core::CandidateRuntimeSnapshot;

use super::summary::{
    account_runtime_warning, local_account_summary, local_source_summary,
    oauth_account_runtime_available, LocalAccountSummaryContext,
};
use zenith_relay_core::protocol::{pooled_source_runtime_available, source_runtime_available};

pub(super) fn project_quota_window_usages(
    accounts: &[LocalAccountRecord],
    telemetry: &TelemetryDb,
    catalog: &PricingCatalog,
    pricing: &PricingContext,
) -> Result<BTreeMap<String, QuotaWindowUsage>, LocalPoolError> {
    let quota_windows = accounts
        .iter()
        .filter_map(|account_record| {
            let window = zenith_relay_core::protocol::api_equivalent_projection_window(
                &account_record.account.quota,
            )?;
            Some((
                account_record.account.id.clone(),
                window.window_start_ms.unwrap_or_default(),
                window.observed_at_ms,
            ))
        })
        .collect::<Vec<_>>();
    let quota_equivalents =
        telemetry.account_api_equivalents_with_pricing(&quota_windows, catalog, pricing)?;
    Ok(accounts
        .iter()
        .filter_map(|account_record| {
            let window = zenith_relay_core::protocol::api_equivalent_projection_window(
                &account_record.account.quota,
            )?;
            let window_start_ms = window.window_start_ms.unwrap_or_default();
            let window_minutes = window.window_minutes.unwrap_or_default();
            Some((
                account_record.account.id.clone(),
                QuotaWindowUsage {
                    kind: window.kind,
                    window_start_ms,
                    observed_at_ms: window.observed_at_ms,
                    window_minutes,
                    api_equivalent: quota_equivalents
                        .get(&account_record.account.id)
                        .copied()
                        .unwrap_or_default(),
                },
            ))
        })
        .collect())
}

pub(super) fn project_source_summaries(
    inputs: &SnapshotInputs,
    routing_order: &[CandidateRuntimeSnapshot],
    equivalents: &UsageEquivalents,
) -> Result<Vec<SourceSummary>, LocalPoolError> {
    inputs
        .sources
        .iter()
        .map(|source_record| {
            let observation = inputs.source_refresh.get(&source_record.id);
            let mut summary = local_source_summary(
                source_record,
                observation.map(|refresh_observation| refresh_observation.revision),
                inputs
                    .source_secret_available
                    .get(&source_record.id)
                    .copied()
                    .unwrap_or(false),
                (inputs.running && source_record.enabled).then(|| {
                    if source_record.in_pool {
                        pooled_source_runtime_available(routing_order, &source_record.id)
                    } else {
                        source_runtime_available(routing_order, &source_record.id)
                    }
                }),
                equivalents
                    .sources
                    .get(&source_record.id)
                    .copied()
                    .unwrap_or_default(),
            )?;
            summary.provider_stats = summary
                .secret_available
                .then(|| {
                    observation.and_then(|refresh_observation| refresh_observation.stats.clone())
                })
                .flatten();
            summary.refresh_state = observation
                .map(|refresh_observation| refresh_observation.state)
                .unwrap_or_default();
            Ok(summary)
        })
        .collect()
}

pub(super) fn project_account_summaries(
    state: &DesktopState,
    inputs: &SnapshotInputs,
    routing_order: &[CandidateRuntimeSnapshot],
    equivalents: &UsageEquivalents,
    quota_window_usages: &BTreeMap<String, QuotaWindowUsage>,
    common_proxy_available: bool,
    snapshot_at_ms: u64,
) -> Result<Vec<AccountSummary>, LocalPoolError> {
    inputs
        .accounts
        .iter()
        .map(|account_record| {
            let mut summary = local_account_summary(
                account_record,
                LocalAccountSummaryContext {
                    settings: &inputs.gateway,
                    credentials: inputs
                        .account_facts
                        .get(&account_record.account.id)
                        .copied()
                        .flatten(),
                    common_proxy_available,
                    api_equivalent: equivalents
                        .accounts
                        .get(&account_record.account.id)
                        .copied()
                        .unwrap_or_default(),
                    quota_window_usage: quota_window_usages
                        .get(&account_record.account.id)
                        .cloned(),
                    now_ms: snapshot_at_ms,
                    refreshing: state.quota_refresh_in_flight(&account_record.account.id)?,
                    runtime_available: (inputs.running && account_record.account.in_pool).then(
                        || {
                            oauth_account_runtime_available(
                                routing_order,
                                &account_record.account.id,
                            )
                            .unwrap_or(false)
                        },
                    ),
                },
            )?;
            summary.refresh_state = inputs
                .account_refresh
                .get(&account_record.account.id)
                .copied()
                .unwrap_or_default();
            Ok(summary)
        })
        .collect()
}

pub(super) fn append_missing_runtime_warnings(
    warnings: &mut Vec<String>,
    inputs: &SnapshotInputs,
    routing_order: &[CandidateRuntimeSnapshot],
    common_proxy_available: bool,
) {
    if !inputs.running {
        return;
    }
    for account_record in &inputs.accounts {
        if account_record.account.enabled
            && account_record.account.in_pool
            && !account_record.account.draining
            && oauth_account_runtime_available(routing_order, &account_record.account.id).is_none()
        {
            warnings.push(account_runtime_warning(
                account_record,
                &inputs.gateway,
                inputs
                    .account_facts
                    .get(&account_record.account.id)
                    .copied()
                    .flatten(),
                common_proxy_available,
            ));
        }
    }
}
