use crate::local_pool::{
    accounts::proxy::{common_proxy_available, common_proxy_pool_id, ProxyPool},
    error::CommandError,
    state::{DesktopState, SnapshotInputs},
};
use crate::platform;
use std::{sync::Arc, time::Instant};
use zenith_relay_core::protocol::{
    apply_model_metadata, apply_pool_model_configuration_with_context, pool_candidate_count,
    pool_model_summaries_with_pricing, pool_pricing_source_summary, DesktopCapabilities,
    GatewaySummary, ModelDisplayProjection, PoolModelConfigurationInput, RuntimeStateSnapshot,
};
use zenith_relay_core::{unix_time_ms, GatewayRuntime, PricingMetadata};

mod members;
#[cfg(test)]
mod performance_tests;
mod summary;
mod timing;
mod worker;
use members::{
    append_missing_runtime_warnings, project_account_summaries, project_quota_window_usages,
    project_source_summaries, AccountProjectionContext,
};
#[cfg(test)]
pub(super) use summary::oauth_account_runtime_available;

/// Build the same prepared runtime projection exposed to the UI. Consumers
/// that generate integrations (for example OpenCode) must use this projection
/// instead of rebuilding a second model catalog from raw pool records.
pub(crate) async fn build_local_runtime_state(
    state: &DesktopState,
) -> Result<RuntimeStateSnapshot, CommandError> {
    let started = Instant::now();
    let mut timing = timing::SnapshotTiming::new();
    // Bound preparation per state before reading inputs. Waiting callers must
    // project fresh inputs, not queue stale inventories or blocking workers.
    let permit = state.runtime_snapshot_permit().await?;
    timing.mark("worker_queue");
    let inputs = state.snapshot_inputs().await?;
    timing.mark("inputs");
    let runtime = state.gateway.runtime().await;
    timing.mark("runtime");
    let state = state.clone();
    worker::run(permit, move || {
        timing.mark("worker_dispatch");
        project_local_runtime_state(&state, inputs, runtime, started, timing)
    })
    .await
}

fn project_local_runtime_state(
    state: &DesktopState,
    inputs: SnapshotInputs,
    runtime: Option<Arc<GatewayRuntime>>,
    started: Instant,
    mut timing: timing::SnapshotTiming,
) -> Result<RuntimeStateSnapshot, CommandError> {
    let running = inputs.running;
    let routing_order = runtime
        .as_ref()
        .map(|runtime| {
            runtime.candidate_runtime_order_for_key(super::super::pool::SYSTEM_GATEWAY_KEY_ID)
        })
        .unwrap_or_default();
    timing.mark("routing");
    let common_proxy_available = common_proxy_available(&inputs.gateway);
    let snapshot_at_ms = unix_time_ms();
    let catalog = state.pricing_catalog();
    let model_metadata = state.model_metadata_catalog();
    let pricing = super::super::pricing_context(
        &inputs.gateway,
        &inputs.sources,
        &inputs.accounts,
        &model_metadata,
    );
    timing.mark("pricing_context");
    let equivalents = state
        .telemetry
        .api_equivalents_with_pricing(&catalog, &pricing)?;
    timing.mark("usage_totals");
    let quota_window_usages =
        project_quota_window_usages(&inputs.accounts, &state.telemetry, &catalog, &pricing)?;
    timing.mark("quota_windows");
    // The pool stores the stable proxy identity separately from account credentials.
    // Use it only for UI provenance; request dispatch still resolves the effective URL.
    let proxy_pool = ProxyPool::load().unwrap_or_default();
    let mut source_summaries =
        project_source_summaries(&inputs, &routing_order, &equivalents, &model_metadata)?;
    timing.mark("source_summaries");
    for source in &mut source_summaries {
        source.resolve_model_prices(&catalog, &pricing);
    }
    timing.mark("source_prices");
    let mut account_summaries = project_account_summaries(AccountProjectionContext {
        state,
        inputs: &inputs,
        routing_order: &routing_order,
        equivalents: &equivalents,
        quota_window_usages: &quota_window_usages,
        proxy_pool: &proxy_pool,
        common_proxy_available,
        snapshot_at_ms,
    })?;
    timing.mark("account_summaries");
    let mut inputs = inputs;
    let mut warnings = std::mem::take(&mut inputs.warnings);
    append_missing_runtime_warnings(
        &mut warnings,
        &inputs,
        &routing_order,
        common_proxy_available,
    );
    let mut models = pool_model_summaries_with_pricing(
        &source_summaries,
        &account_summaries,
        &inputs.gateway.hidden_models,
        &catalog,
        &pricing,
    );
    timing.mark("pool_models");
    apply_model_metadata(&mut models, &model_metadata);
    apply_pool_model_configuration_with_context(
        &mut models,
        PoolModelConfigurationInput {
            sources: &source_summaries,
            accounts: &account_summaries,
            model_price_overrides: &inputs.gateway.model_price_overrides,
            model_reasoning_allowed_levels: &inputs.gateway.model_reasoning_allowed_levels,
            model_context_policies: &inputs.gateway.model_context_policies,
            model_service_tier_overrides: &inputs.gateway.model_service_tier_overrides,
            runtime: runtime.as_deref(),
        },
    );
    timing.mark("model_configuration");
    let mut display_projection = ModelDisplayProjection::new(&model_metadata);
    display_projection.apply_model_order(&mut models, &inputs.gateway.model_display_order);
    timing.mark("display_order");
    display_projection.apply_member_order(
        &mut source_summaries,
        &mut account_summaries,
        &inputs.gateway.model_display_order,
    );
    let visible_model_ids = models
        .iter()
        .filter(|model| model.enabled && !model.protocol_routes.is_empty())
        .map(|model| model.id.clone())
        .collect();
    let candidate_count = pool_candidate_count(&source_summaries, &account_summaries);
    let base_url = format!(
        "http://{}:{}/v1",
        inputs.gateway.client_host, inputs.gateway.port
    );
    let pricing_metadata = PricingMetadata::for_catalog_with_status(
        &catalog,
        state.pricing_status(),
        pool_pricing_source_summary(&source_summaries, &account_summaries, &catalog, &pricing),
        equivalents
            .accounts
            .values()
            .chain(equivalents.sources.values())
            .map(|equivalent_usage| equivalent_usage.unpriced_tokens)
            .sum(),
    );
    let response = RuntimeStateSnapshot {
        schema_version: crate::local_pool::models::CURRENT_SCHEMA_VERSION,
        gateway: GatewaySummary {
            tool_policy: inputs.gateway.tool_policy.clone(),
            pool_routing: Some(zenith_relay_core::protocol::pool_routing_summary(
                inputs.gateway.pool_routing.as_ref(),
                &source_summaries,
                &account_summaries,
            )),
            running,
            base_url,
            candidate_count,
            visible_model_ids,
            max_retry_candidates: inputs.gateway.max_retry_candidates,
            default_service_tier: inputs.gateway.default_service_tier,
            image_base_model: inputs.gateway.image_base_model.clone(),
            models,
            model_catalog: display_projection.member_catalog(
                &source_summaries,
                &account_summaries,
                &inputs.gateway.model_display_order,
            ),
            common_proxy_configured: inputs.gateway.common_proxy_configured,
            common_proxy_available,
            common_proxy_id: common_proxy_pool_id(&inputs.gateway, &proxy_pool),
            account_proxy_required: inputs.gateway.account_proxy_required,
            quota_request_timeout_seconds: inputs.gateway.quota_request_timeout_seconds,
            chatgpt_interface_quota_reserve_basis_points: Some(
                inputs.gateway.chatgpt_interface_quota_reserve_basis_points,
            ),
            codex_background_tasks_enabled: inputs.gateway.codex_background_tasks_enabled,
            codex_memory_extraction_enabled: inputs.gateway.codex_memory_extraction_enabled,
            codex_memory_consolidation_enabled: inputs.gateway.codex_memory_consolidation_enabled,
            codex_websockets_enabled: inputs.gateway.codex_websockets_enabled,
            chatgpt_retry_until_available: inputs.gateway.chatgpt_retry_until_available,
            block_degraded_routes_enabled: inputs.gateway.block_degraded_routes_enabled,
            routing_order,
        },
        platform: platform::platform_name().to_string(),
        capabilities: DesktopCapabilities::current(),
        sources: source_summaries,
        accounts: account_summaries,
        automations: inputs.automations.tasks,
        wake_history: inputs.automations.state.history().iter().cloned().collect(),
        warnings,
        pricing: pricing_metadata,
    };
    timing.mark("catalog_projection");
    timing.finish(state, started.elapsed());
    Ok(response)
}
