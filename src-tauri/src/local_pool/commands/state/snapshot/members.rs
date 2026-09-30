use std::collections::BTreeMap;

use crate::local_pool::error::LocalPoolError;
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::{DesktopState, LocalRuntimeInputs};
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
        .filter_map(|record| {
            let window = zenith_relay_core::protocol::api_equivalent_projection_window(
                &record.account.quota,
            )?;
            Some((
                record.account.id.clone(),
                window.window_start_ms.unwrap_or_default(),
                window.observed_at_ms,
            ))
        })
        .collect::<Vec<_>>();
    let quota_equivalents =
        telemetry.account_api_equivalents_with_pricing(&quota_windows, catalog, pricing)?;
    Ok(accounts
        .iter()
        .filter_map(|record| {
            let window = zenith_relay_core::protocol::api_equivalent_projection_window(
                &record.account.quota,
            )?;
            let window_start_ms = window.window_start_ms.unwrap_or_default();
            let window_minutes = window.window_minutes.unwrap_or_default();
            Some((
                record.account.id.clone(),
                QuotaWindowUsage {
                    kind: window.kind,
                    window_start_ms,
                    observed_at_ms: window.observed_at_ms,
                    window_minutes,
                    api_equivalent: quota_equivalents
                        .get(&record.account.id)
                        .copied()
                        .unwrap_or_default(),
                },
            ))
        })
        .collect())
}

pub(super) fn project_source_summaries(
    inputs: &LocalRuntimeInputs,
    routing_order: &[CandidateRuntimeSnapshot],
    equivalents: &UsageEquivalents,
) -> Result<Vec<SourceSummary>, LocalPoolError> {
    inputs
        .sources
        .iter()
        .map(|record| {
            let observation = inputs.source_refresh.get(&record.id);
            let mut summary = local_source_summary(
                record,
                observation.map(|value| value.revision),
                inputs
                    .source_api_keys
                    .get(&record.id)
                    .and_then(Option::as_ref)
                    .is_some(),
                (inputs.running && record.enabled).then(|| {
                    if record.in_pool {
                        pooled_source_runtime_available(routing_order, &record.id)
                    } else {
                        source_runtime_available(routing_order, &record.id)
                    }
                }),
                equivalents
                    .sources
                    .get(&record.id)
                    .copied()
                    .unwrap_or_default(),
            )?;
            summary.provider_stats = summary
                .secret_available
                .then(|| observation.and_then(|value| value.stats.clone()))
                .flatten();
            summary.refresh_state = observation.map(|value| value.state).unwrap_or_default();
            Ok(summary)
        })
        .collect()
}

pub(super) fn project_account_summaries(
    state: &DesktopState,
    inputs: &LocalRuntimeInputs,
    routing_order: &[CandidateRuntimeSnapshot],
    equivalents: &UsageEquivalents,
    quota_window_usages: &BTreeMap<String, QuotaWindowUsage>,
    common_proxy_available: bool,
    snapshot_at_ms: u64,
) -> Result<Vec<AccountSummary>, LocalPoolError> {
    inputs
        .accounts
        .iter()
        .map(|record| {
            let mut summary = local_account_summary(
                record,
                LocalAccountSummaryContext {
                    settings: &inputs.gateway,
                    credentials: inputs
                        .account_credentials
                        .get(&record.account.id)
                        .and_then(Option::as_ref),
                    common_proxy_available,
                    api_equivalent: equivalents
                        .accounts
                        .get(&record.account.id)
                        .copied()
                        .unwrap_or_default(),
                    quota_window_usage: quota_window_usages.get(&record.account.id).cloned(),
                    now_ms: snapshot_at_ms,
                    refreshing: state.quota_refresh_in_flight(&record.account.id)?,
                    runtime_available: (inputs.running && record.account.in_pool).then(|| {
                        oauth_account_runtime_available(routing_order, &record.account.id)
                            .unwrap_or(false)
                    }),
                },
            )?;
            summary.refresh_state = inputs
                .account_refresh
                .get(&record.account.id)
                .copied()
                .unwrap_or_default();
            Ok(summary)
        })
        .collect()
}

pub(super) fn append_missing_runtime_warnings(
    warnings: &mut Vec<String>,
    inputs: &LocalRuntimeInputs,
    routing_order: &[CandidateRuntimeSnapshot],
) {
    if !inputs.running {
        return;
    }
    for record in &inputs.accounts {
        if record.account.enabled
            && record.account.in_pool
            && !record.account.draining
            && oauth_account_runtime_available(routing_order, &record.account.id).is_none()
        {
            warnings.push(account_runtime_warning(
                record,
                &inputs.gateway,
                &record.account.id,
                inputs
                    .account_credentials
                    .get(&record.account.id)
                    .and_then(Option::as_ref),
            ));
        }
    }
}
