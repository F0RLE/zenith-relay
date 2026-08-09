use super::{
    account_runtime::{
        account_proxy_status, account_summary, common_proxy_available, source_summary,
    },
    AccountCredential, AppState, ServerAccountRecord, SourceRecord,
};
use crate::{
    state::{identity_hint, now_ms, SERVER_SCHEMA_VERSION},
    store::configuration_revision,
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::atomic::Ordering,
};
use zenith_relay_core::{
    protocol::{
        apply_model_reasoning_summary, model_has_native_account_route, source_runtime_available,
        AccountSummary, GatewaySummary, ModelSummary, OperationalStatus, ProxyMode,
        RuntimeStateSnapshot, RuntimeTargetSummary, SourceSummary,
    },
    quota::{attach_quota_plan_benchmarks, quota_plan_benchmarks, quota_valuation_revision},
    ApiEquivalentSummary, ApiModelPriceOverride, CandidateRuntimeSnapshot, DefaultServiceTier,
    GatewayRuntime, WireApi, QUOTA_STALE_AFTER_MS,
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
    let configuration_revision = configuration_revision(&state.store.configuration_settings()?)?;
    let equivalents = state.store.api_equivalents()?;
    let runtime = state.runtime()?;
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
    let mut account_summaries = account_summaries(
        state,
        &accounts,
        proxy_settings,
        routing_policy.default_service_tier,
        &equivalents,
        &mut warnings,
    )?;
    attach_account_plan_benchmarks(
        &accounts,
        &mut account_summaries,
        routing_policy.default_service_tier,
    );
    let models = model_summaries(
        &source_summaries,
        &account_summaries,
        &hidden_models,
        &model_price_overrides,
        &model_reasoning_allowed_levels,
        runtime.as_deref(),
    );
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
            candidate_count: candidate_count(&source_summaries, &account_summaries),
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
            Ok(source_summary(
                record,
                secret_available,
                (running && record.enabled)
                    .then(|| source_runtime_available(routing_order, &record.id)),
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
    default_service_tier: DefaultServiceTier,
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
            Ok(account_summary(
                record,
                secret_available,
                proxy_mode,
                proxy_available,
                equivalents
                    .get(&identity_hint(&record.id))
                    .copied()
                    .unwrap_or_default(),
                default_service_tier,
                QUOTA_STALE_AFTER_MS,
            ))
        })
        .collect()
}

fn attach_account_plan_benchmarks(
    records: &[ServerAccountRecord],
    summaries: &mut [AccountSummary],
    default_service_tier: DefaultServiceTier,
) {
    let economics_revision = quota_valuation_revision();
    let plan_benchmarks = quota_plan_benchmarks(
        records
            .iter()
            .map(|account| (account.id.as_str(), &account.economics)),
        now_ms(),
        economics_revision,
    );
    for (record, summary) in records.iter().zip(summaries) {
        attach_quota_plan_benchmarks(
            &mut summary.economics,
            "chatgpt",
            record.subscription.plan_type.as_deref(),
            &record.quota,
            default_service_tier,
            economics_revision,
            &plan_benchmarks,
        );
    }
}

fn model_summaries(
    source_summaries: &[SourceSummary],
    account_summaries: &[AccountSummary],
    hidden_models: &[String],
    model_price_overrides: &BTreeMap<String, ApiModelPriceOverride>,
    model_reasoning_allowed_levels: &BTreeMap<String, Vec<String>>,
    runtime: Option<&GatewayRuntime>,
) -> Vec<ModelSummary> {
    let mut models = zenith_relay_core::protocol::pool_model_summaries(
        source_summaries,
        account_summaries,
        hidden_models,
    );
    for model in &mut models {
        let model_id = model.id.clone();
        if let Some(price) = model_price_overrides.get(&model_id.to_ascii_lowercase()) {
            model.input_micro_usd_per_million = Some(price.input_micro_usd_per_million);
            model.cached_input_micro_usd_per_million = Some(
                price
                    .cached_input_micro_usd_per_million
                    .unwrap_or(price.input_micro_usd_per_million),
            );
            model.cache_write_5m_micro_usd_per_million = price.cache_write_5m_micro_usd_per_million;
            model.cache_write_1h_micro_usd_per_million = price.cache_write_1h_micro_usd_per_million;
            model.output_micro_usd_per_million = Some(price.output_micro_usd_per_million);
            model.custom_price = true;
        }
        apply_model_reasoning_summary(
            model,
            runtime
                .map(|runtime| runtime.confirmed_source_reasoning_levels(&model_id))
                .unwrap_or_default(),
            model_reasoning_allowed_levels
                .get(&model_id.to_ascii_lowercase())
                .map(Vec::as_slice),
            model_has_native_account_route(account_summaries, &model_id),
        );
    }
    models
}

fn candidate_count(sources: &[SourceSummary], accounts: &[AccountSummary]) -> usize {
    sources
        .iter()
        .filter(|record| {
            record.in_pool
                && record.supports_wire_api(WireApi::Responses)
                && record.operational_status == OperationalStatus::Rotation
        })
        .count()
        + accounts
            .iter()
            .filter(|record| {
                record.in_pool && record.operational_status == OperationalStatus::Rotation
            })
            .count()
}
