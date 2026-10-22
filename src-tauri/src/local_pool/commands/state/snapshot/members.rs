use std::collections::BTreeMap;

use crate::local_pool::accounts::proxy::ProxyPool;
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
    reference_catalog: &zenith_relay_core::model_metadata::ModelMetadataCatalog,
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
                reference_catalog,
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

pub(super) struct AccountProjectionContext<'a> {
    pub(super) state: &'a DesktopState,
    pub(super) inputs: &'a SnapshotInputs,
    pub(super) routing_order: &'a [CandidateRuntimeSnapshot],
    pub(super) equivalents: &'a UsageEquivalents,
    pub(super) quota_window_usages: &'a BTreeMap<String, QuotaWindowUsage>,
    pub(super) proxy_pool: &'a ProxyPool,
    pub(super) common_proxy_available: bool,
    pub(super) snapshot_at_ms: u64,
}

pub(super) fn project_account_summaries(
    context: AccountProjectionContext<'_>,
) -> Result<Vec<AccountSummary>, LocalPoolError> {
    let AccountProjectionContext {
        state,
        inputs,
        routing_order,
        equivalents,
        quota_window_usages,
        proxy_pool,
        common_proxy_available,
        snapshot_at_ms,
    } = context;
    inputs
        .accounts
        .iter()
        .map(|account_record| {
            let mut summary = local_account_summary(
                account_record,
                LocalAccountSummaryContext {
                    settings: &inputs.gateway,
                    proxy_id: proxy_pool.proxy_id_for_account(&account_record.account.id),
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
                    runtime_quota_wait: routing_order.iter().any(|candidate| {
                        candidate.kind == zenith_relay_core::CandidateKind::OAuthAccount
                            && candidate.candidate_id == account_record.account.id
                            && candidate.quota_wait
                    }),
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
