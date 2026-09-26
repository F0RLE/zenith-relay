use super::{
    account_runtime::{
        account_proxy_status, account_summary, common_proxy_available, source_summary,
        AccountSummaryInputs,
    },
    AccountCredential, AppState, ServerAccountRecord, SourceRecord,
};
use crate::{
    state::{identity_hint, SERVER_SCHEMA_VERSION},
    store::{configuration_revision, AccountRefreshFence, SourceRefreshFence},
};
use std::{collections::HashMap, sync::atomic::Ordering};
use zenith_relay_core::error_codes;
use zenith_relay_core::{
    pricing::{PricingCatalog, PricingContext, PricingMetadata},
    protocol::{
        apply_model_display_order_with_catalog, apply_model_metadata,
        apply_pool_model_configuration, pool_candidate_count, pool_model_summaries_with_pricing,
        pool_pricing_source_summary, pooled_source_runtime_available, source_runtime_available,
        AccountRefreshState, AccountSummary, GatewaySummary, ProxyMode, QuotaWindowUsage,
        RefreshStatus, RuntimeStateSnapshot, RuntimeTargetSummary, SourceRefreshState,
        SourceSummary, UsageQuery,
    },
    scheduler::refresh::RefreshKind,
    ApiEquivalentSummary, CandidateRuntimeSnapshot, QUOTA_STALE_AFTER_MS,
};

#[derive(Clone, Copy)]
struct AccountProxySettings {
    common_configured: bool,
    common_available: bool,
    required: bool,
}

struct AccountSnapshotInputs<'a> {
    proxy_settings: AccountProxySettings,
    equivalents: &'a HashMap<String, ApiEquivalentSummary>,
    pricing_catalog: &'a PricingCatalog,
    pricing_context: &'a PricingContext,
    basis_points_enabled: bool,
}

pub(super) fn build(state: &AppState) -> Result<RuntimeStateSnapshot, String> {
    let source_scopes = state.store.source_refresh_scopes()?;
    let accounts = state.store.account_refresh_scopes()?;
    let common_proxy_configured = state.store.common_proxy_configured()?;
    let common_proxy_id = state.store.common_proxy_id()?;
    let common_proxy_available = common_proxy_available(state, common_proxy_configured);
    let account_proxy_required = state.store.account_proxy_required()?;
    let proxy_settings = AccountProxySettings {
        common_configured: common_proxy_configured,
        common_available: common_proxy_available,
        required: account_proxy_required,
    };
    let quota_request_timeout_seconds = state.store.quota_request_timeout_seconds()?;
    let routing_policy = state.store.routing_policy()?;
    let hidden_models = state.store.hidden_models()?;
    let model_price_overrides = state.store.model_price_overrides()?;
    let model_reasoning_allowed_levels = state.store.model_reasoning_allowed_levels()?;
    let model_service_tier_overrides = state.store.model_service_tier_overrides()?;
    let model_display_order = state.store.model_display_order()?;
    let configuration_revision = configuration_revision(&state.store.configuration_settings()?)?;
    let pricing_catalog = state.pricing_catalog();
    let model_metadata = state.model_metadata_catalog();
    let pricing_context = state.pricing_context()?;
    let equivalents = state
        .store
        .api_equivalents_with_pricing(&pricing_catalog, &pricing_context)?;
    let runtime = state.runtime()?;
    let codex_background_tasks_enabled = state.store.codex_background_tasks_enabled()?;
    let codex_websockets_enabled = state.store.codex_websockets_enabled()?;
    let chatgpt_retry_until_available = state.store.chatgpt_retry_until_available()?;
    let running = state.store.gateway_enabled()? && runtime.is_some();
    let routing_order = runtime
        .as_ref()
        .map(|runtime| runtime.candidate_runtime_order_for_key(crate::state::SYSTEM_GATEWAY_KEY_ID))
        .unwrap_or_default();
    let mut warnings = usage_warnings(state);
    let mut source_summaries = source_summaries(
        state,
        &source_scopes,
        running,
        &routing_order,
        &equivalents,
        &mut warnings,
    )?;
    let mut account_summaries = account_summaries(
        state,
        &accounts,
        AccountSnapshotInputs {
            proxy_settings,
            equivalents: &equivalents,
            pricing_catalog: &pricing_catalog,
            pricing_context: &pricing_context,
            basis_points_enabled: routing_policy.basis_points_enabled,
        },
        &mut warnings,
    )?;
    for account in &mut account_summaries {
        let available = (running && account.in_pool).then(|| {
            routing_order.iter().any(|candidate| {
                candidate.kind == zenith_relay_core::CandidateKind::OAuthAccount
                    && candidate.candidate_id == account.id
                    && candidate.available
            })
        });
        account.operational_status = account.operational_status.with_runtime_available(available);
    }
    let mut models = pool_model_summaries_with_pricing(
        &source_summaries,
        &account_summaries,
        &hidden_models,
        &pricing_catalog,
        &pricing_context,
    );
    apply_model_metadata(&mut models, &model_metadata);
    apply_pool_model_configuration(
        &mut models,
        &source_summaries,
        &account_summaries,
        &model_price_overrides,
        &model_reasoning_allowed_levels,
        &model_service_tier_overrides,
        runtime.as_deref(),
    );
    apply_model_display_order_with_catalog(&mut models, &model_display_order, &model_metadata);
    zenith_relay_core::protocol::apply_member_model_display_order(
        &mut source_summaries,
        &mut account_summaries,
        &model_display_order,
        &model_metadata,
    );
    let visible_model_ids = models
        .iter()
        .filter(|model| model.enabled && !model.protocol_routes.is_empty())
        .map(|model| model.id.clone())
        .collect();
    let pricing_metadata = PricingMetadata::for_catalog_with_status(
        &pricing_catalog,
        state.pricing_status(),
        pool_pricing_source_summary(
            &source_summaries,
            &account_summaries,
            &pricing_catalog,
            &pricing_context,
        ),
        equivalents
            .values()
            .map(|value| value.unpriced_tokens)
            .sum(),
    );

    Ok(RuntimeStateSnapshot {
        schema_version: SERVER_SCHEMA_VERSION,
        configuration_revision: Some(configuration_revision),
        runtime_target: RuntimeTargetSummary {
            kind: "remote".to_string(),
            connected: true,
            origin: Some(state.config.public_base_url.origin().ascii_serialization()),
            server_id: Some(state.capabilities.server_id.clone()),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
        gateway: GatewaySummary {
            tool_policy: routing_policy.tool_policy.unwrap_or_default(),
            basis_points_enabled: routing_policy.basis_points_enabled,
            pool_routing: Some(zenith_relay_core::protocol::pool_routing_summary(
                routing_policy.pool_routing.as_ref(),
                &source_summaries,
                &account_summaries,
            )),
            running,
            base_url: format!(
                "{}/v1",
                state.config.public_base_url.as_str().trim_end_matches('/')
            ),
            candidate_count: pool_candidate_count(&source_summaries, &account_summaries),
            visible_model_ids,
            max_retry_candidates: routing_policy.max_retry_candidates,
            default_service_tier: routing_policy.default_service_tier,
            image_base_model: routing_policy.image_base_model,
            models,
            model_catalog: zenith_relay_core::protocol::member_model_catalog(
                &source_summaries,
                &account_summaries,
                &model_metadata,
            ),
            common_proxy_configured: proxy_settings.common_configured,
            common_proxy_available: proxy_settings.common_available,
            common_proxy_id,
            account_proxy_required: proxy_settings.required,
            quota_request_timeout_seconds,
            chatgpt_interface_quota_reserve_basis_points: None,
            codex_background_tasks_enabled,
            codex_websockets_enabled,
            chatgpt_retry_until_available,
            routing_order,
        },
        platform: std::env::consts::OS.to_string(),
        capabilities: state.capabilities.clone(),
        sources: source_summaries,
        accounts: account_summaries,
        automations: state.store.wake_tasks()?,
        wake_history: state
            .store
            .wake_state()?
            .history()
            .iter()
            .cloned()
            .collect(),
        warnings,
        pricing: pricing_metadata,
    })
}

fn usage_warnings(state: &AppState) -> Vec<String> {
    (state.failed_usage_writes.load(Ordering::Relaxed) > 0)
        .then(|| error_codes::USAGE_PERSISTENCE_FAILED.to_string())
        .into_iter()
        .collect()
}

fn source_summaries(
    state: &AppState,
    records: &[(SourceRecord, SourceRefreshFence)],
    running: bool,
    routing_order: &[CandidateRuntimeSnapshot],
    equivalents: &HashMap<String, ApiEquivalentSummary>,
    warnings: &mut Vec<String>,
) -> Result<Vec<SourceSummary>, String> {
    records
        .iter()
        .map(|(record, fence)| {
            let secret_available = state.vault.load(&record.secret_ref)?.is_some();
            if !secret_available {
                warnings.push(format!("source_secret_missing:{}", record.id));
            }
            let runtime_available = (running && record.enabled).then(|| {
                if record.in_pool {
                    pooled_source_runtime_available(routing_order, &record.id)
                } else {
                    source_runtime_available(routing_order, &record.id)
                }
            });
            let mut summary = source_summary(
                record,
                secret_available,
                runtime_available,
                equivalents
                    .get(&identity_hint(&record.id))
                    .copied()
                    .unwrap_or_default(),
            );
            summary.refresh_revision = Some(fence.revision());
            summary.refresh_state = SourceRefreshState {
                models: RefreshStatus::from_evidence(
                    state
                        .refresh
                        .freshness(&fence.identity(), RefreshKind::Models),
                    !record.models.is_empty(),
                ),
                balance: RefreshStatus::from_evidence(
                    state
                        .refresh
                        .freshness(&fence.identity(), RefreshKind::Balance),
                    false,
                ),
            };
            if secret_available {
                summary.provider_stats =
                    crate::jobs::cached_source_stats(state, fence, &record.base_url);
            }
            Ok(summary)
        })
        .collect()
}

fn account_summaries(
    state: &AppState,
    records: &[(ServerAccountRecord, AccountRefreshFence)],
    inputs: AccountSnapshotInputs<'_>,
    warnings: &mut Vec<String>,
) -> Result<Vec<AccountSummary>, String> {
    records
        .iter()
        .map(|(record, fence)| {
            let secret = state.vault.load(&record.secret_ref)?;
            let secret_available = secret.is_some();
            if !secret_available {
                warnings.push(format!("account_secret_missing:{}", record.id));
            }
            let credential = secret
                .as_deref()
                .and_then(|value| serde_json::from_str::<AccountCredential>(value).ok());
            let basis_points_available = credential
                .as_ref()
                .is_some_and(|value| value.has_oauth() && !value.is_agent_identity());
            let (proxy_mode, proxy_available) = credential
                .as_ref()
                .map(|credential| {
                    account_proxy_status(
                        state,
                        record,
                        credential,
                        inputs.proxy_settings.common_configured,
                        inputs.proxy_settings.common_available,
                        inputs.proxy_settings.required,
                    )
                })
                .unwrap_or((ProxyMode::Direct, false));
            let quota_window_usage = account_quota_window_usage(
                state,
                record,
                inputs.pricing_catalog,
                inputs.pricing_context,
            )?;
            let mut summary = account_summary(
                record,
                AccountSummaryInputs {
                    secret_available,
                    basis_points_available,
                    basis_points_enabled: inputs.basis_points_enabled,
                    proxy_mode,
                    proxy_available,
                    api_equivalent: inputs
                        .equivalents
                        .get(&identity_hint(&record.id))
                        .copied()
                        .unwrap_or_default(),
                    quota_window_usage,
                    quota_stale_after_ms: QUOTA_STALE_AFTER_MS,
                },
            );
            summary.refresh_state = AccountRefreshState {
                models: RefreshStatus::from_evidence(
                    state
                        .refresh
                        .freshness(&fence.identity(), RefreshKind::Models),
                    !record.models.is_empty(),
                ),
                quota: RefreshStatus::from_evidence(
                    state
                        .refresh
                        .freshness(&fence.identity(), RefreshKind::Quota),
                    record.quota.updated_at_ms.is_some(),
                ),
            };
            Ok(summary)
        })
        .collect()
}

fn account_quota_window_usage(
    state: &AppState,
    record: &ServerAccountRecord,
    pricing_catalog: &PricingCatalog,
    pricing_context: &PricingContext,
) -> Result<Option<QuotaWindowUsage>, String> {
    let Some(window) = zenith_relay_core::protocol::api_equivalent_projection_window(&record.quota)
    else {
        return Ok(None);
    };
    let window_start_ms = window.window_start_ms.unwrap_or_default();
    let window_minutes = window.window_minutes.unwrap_or_default();
    let usage = state.store.usage_page_with_pricing(
        &UsageQuery {
            page: 1,
            page_size: 1,
            from_ms: Some(window_start_ms),
            to_ms: Some(window.observed_at_ms),
            source_or_account_query: Some(identity_hint(&record.id)),
            ..UsageQuery::default()
        },
        pricing_catalog,
        pricing_context,
    )?;
    Ok(Some(QuotaWindowUsage {
        kind: window.kind,
        window_start_ms,
        observed_at_ms: window.observed_at_ms,
        window_minutes,
        api_equivalent: usage.totals.api_equivalent,
    }))
}
