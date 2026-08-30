use super::{
    account_runtime::{
        account_proxy_status, account_summary, common_proxy_available, source_summary,
    },
    AccountCredential, AppState, ServerAccountRecord, SourceRecord,
};
use crate::{
    state::{identity_hint, SERVER_SCHEMA_VERSION},
    store::configuration_revision,
};
use std::{collections::HashMap, sync::atomic::Ordering};
use zenith_relay_core::{
    protocol::{
        apply_model_display_order, apply_pool_model_configuration, pool_candidate_count,
        pooled_source_runtime_available, source_runtime_available, AccountSummary, GatewaySummary,
        ProxyMode, QuotaWindowUsage, RuntimeStateSnapshot, RuntimeTargetSummary, SourceSummary,
        UsageQuery,
    },
    ApiEquivalentSummary, CandidateRuntimeSnapshot, QUOTA_STALE_AFTER_MS,
};

#[derive(Clone, Copy)]
struct AccountProxySettings {
    common_configured: bool,
    common_available: bool,
    required: bool,
}

pub(super) fn build(state: &AppState) -> Result<RuntimeStateSnapshot, String> {
    let sources = state.store.sources()?;
    let accounts = state.store.accounts()?;
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
    let equivalents = state.store.api_equivalents()?;
    let runtime = state.runtime()?;
    let codex_background_tasks_enabled = state.store.codex_background_tasks_enabled()?;
    let codex_websockets_enabled = state.store.codex_websockets_enabled()?;
    let running = state.store.gateway_enabled()? && runtime.is_some();
    let routing_order = runtime
        .as_ref()
        .map(|runtime| runtime.candidate_runtime_order())
        .unwrap_or_default();
    let mut warnings = usage_warnings(state);
    let source_summaries = source_summaries(
        state,
        &sources,
        running,
        &routing_order,
        &equivalents,
        &mut warnings,
    )?;
    let account_summaries = account_summaries(
        state,
        &accounts,
        proxy_settings,
        &equivalents,
        &mut warnings,
    )?;
    let mut models = zenith_relay_core::protocol::pool_model_summaries(
        &source_summaries,
        &account_summaries,
        &hidden_models,
    );
    apply_pool_model_configuration(
        &mut models,
        &source_summaries,
        &account_summaries,
        &model_price_overrides,
        &model_reasoning_allowed_levels,
        &model_service_tier_overrides,
        runtime.as_deref(),
    );
    apply_model_display_order(&mut models, &model_display_order);
    let visible_model_ids = models
        .iter()
        .filter(|model| model.enabled)
        .map(|model| model.id.clone())
        .collect();

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
            running,
            base_url: format!(
                "{}/v1",
                state.config.public_base_url.as_str().trim_end_matches('/')
            ),
            candidate_count: pool_candidate_count(&source_summaries, &account_summaries),
            visible_model_ids,
            max_retry_candidates: routing_policy.max_retry_candidates,
            cooldown_after_failures: routing_policy.cooldown_after_failures,
            keep_last_candidate_available: routing_policy.keep_last_candidate_available,
            routing_strategy: routing_policy.routing_strategy,
            subscription_plan_order: routing_policy.subscription_plan_order,
            default_service_tier: routing_policy.default_service_tier,
            image_base_model: routing_policy.image_base_model,
            models,
            common_proxy_configured: proxy_settings.common_configured,
            common_proxy_available: proxy_settings.common_available,
            common_proxy_id,
            account_proxy_required: proxy_settings.required,
            quota_request_timeout_seconds,
            chatgpt_interface_quota_reserve_basis_points: None,
            codex_background_tasks_enabled,
            codex_websockets_enabled,
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
    })
}

fn usage_warnings(state: &AppState) -> Vec<String> {
    (state.failed_usage_writes.load(Ordering::Relaxed) > 0)
        .then(|| "usage_persistence_failed".to_string())
        .into_iter()
        .collect()
}

fn source_summaries(
    state: &AppState,
    records: &[SourceRecord],
    running: bool,
    routing_order: &[CandidateRuntimeSnapshot],
    equivalents: &HashMap<String, ApiEquivalentSummary>,
    warnings: &mut Vec<String>,
) -> Result<Vec<SourceSummary>, String> {
    records
        .iter()
        .map(|record| {
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
            Ok(source_summary(
                record,
                secret_available,
                runtime_available,
                equivalents
                    .get(&identity_hint(&record.id))
                    .copied()
                    .unwrap_or_default(),
            ))
        })
        .collect()
}

fn account_summaries(
    state: &AppState,
    records: &[ServerAccountRecord],
    proxy_settings: AccountProxySettings,
    equivalents: &HashMap<String, ApiEquivalentSummary>,
    warnings: &mut Vec<String>,
) -> Result<Vec<AccountSummary>, String> {
    records
        .iter()
        .map(|record| {
            let secret = state.vault.load(&record.secret_ref)?;
            let secret_available = secret.is_some();
            if !secret_available {
                warnings.push(format!("account_secret_missing:{}", record.id));
            }
            let (proxy_mode, proxy_available) = secret
                .as_deref()
                .and_then(|value| serde_json::from_str::<AccountCredential>(value).ok())
                .map(|credential| {
                    account_proxy_status(
                        state,
                        record,
                        &credential,
                        proxy_settings.common_configured,
                        proxy_settings.common_available,
                        proxy_settings.required,
                    )
                })
                .unwrap_or((ProxyMode::Direct, false));
            let quota_window_usage = account_quota_window_usage(state, record)?;
            Ok(account_summary(
                record,
                secret_available,
                proxy_mode,
                proxy_available,
                equivalents
                    .get(&identity_hint(&record.id))
                    .copied()
                    .unwrap_or_default(),
                quota_window_usage,
                QUOTA_STALE_AFTER_MS,
            ))
        })
        .collect()
}

fn account_quota_window_usage(
    state: &AppState,
    record: &ServerAccountRecord,
) -> Result<Option<QuotaWindowUsage>, String> {
    let Some(window) = zenith_relay_core::protocol::api_equivalent_projection_window(&record.quota)
    else {
        return Ok(None);
    };
    let window_start_ms = window.window_start_ms.unwrap_or_default();
    let window_minutes = window.window_minutes.unwrap_or_default();
    let usage = state.store.usage_page(&UsageQuery {
        page: 1,
        page_size: 1,
        from_ms: Some(window_start_ms),
        to_ms: Some(window.observed_at_ms),
        source_or_account_query: Some(identity_hint(&record.id)),
        ..UsageQuery::default()
    })?;
    Ok(Some(QuotaWindowUsage {
        kind: window.kind,
        window_start_ms,
        observed_at_ms: window.observed_at_ms,
        window_minutes,
        api_equivalent: usage.totals.api_equivalent,
    }))
}
