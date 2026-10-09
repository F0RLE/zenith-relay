use crate::local_pool::{
    accounts::proxy::common_proxy_available, error::CommandError, state::DesktopState,
};
use crate::platform;
use std::time::Instant;
use zenith_relay_core::protocol::{
    apply_model_display_order_with_catalog, apply_model_metadata, apply_pool_model_configuration,
    pool_candidate_count, pool_model_summaries_with_pricing, pool_pricing_source_summary,
    Capabilities, GatewaySummary, RuntimeStateSnapshot, RuntimeTargetSummary,
};
use zenith_relay_core::{unix_time_ms, PricingMetadata};

mod members;
mod summary;
use members::{
    append_missing_runtime_warnings, project_account_summaries, project_quota_window_usages,
    project_source_summaries,
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
    let inputs = state.snapshot_inputs().await?;
    let running = inputs.running;
    let runtime = state.gateway.runtime().await;
    let routing_order = runtime
        .as_ref()
        .map(|runtime| {
            runtime.candidate_runtime_order_for_key(super::super::pool::SYSTEM_GATEWAY_KEY_ID)
        })
        .unwrap_or_default();
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
    let equivalents = state
        .telemetry
        .api_equivalents_with_pricing(&catalog, &pricing)?;
    let quota_window_usages =
        project_quota_window_usages(&inputs.accounts, &state.telemetry, &catalog, &pricing)?;
    let mut source_summaries =
        project_source_summaries(&inputs, &routing_order, &equivalents, &model_metadata)?;
    let mut account_summaries = project_account_summaries(
        state,
        &inputs,
        &routing_order,
        &equivalents,
        &quota_window_usages,
        common_proxy_available,
        snapshot_at_ms,
    )?;
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
    apply_model_metadata(&mut models, &model_metadata);
    apply_pool_model_configuration(
        &mut models,
        &source_summaries,
        &account_summaries,
        &inputs.gateway.model_price_overrides,
        &inputs.gateway.model_reasoning_allowed_levels,
        &inputs.gateway.model_service_tier_overrides,
        runtime.as_deref(),
    );
    apply_model_display_order_with_catalog(
        &mut models,
        &inputs.gateway.model_display_order,
        &model_metadata,
    );
    zenith_relay_core::protocol::apply_member_model_display_order(
        &mut source_summaries,
        &mut account_summaries,
        &inputs.gateway.model_display_order,
        &model_metadata,
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
        configuration_revision: None,
        runtime_target: RuntimeTargetSummary {
            kind: "local".to_string(),
            connected: running,
            origin: Some(format!(
                "http://{}:{}",
                inputs.gateway.client_host, inputs.gateway.port
            )),
            server_id: None,
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        },
        gateway: GatewaySummary {
            tool_policy: inputs.gateway.tool_policy.clone(),
            basis_points_enabled: false,
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
            model_catalog: zenith_relay_core::protocol::member_model_catalog(
                &source_summaries,
                &account_summaries,
                &model_metadata,
            ),
            common_proxy_configured: inputs.gateway.common_proxy_configured,
            common_proxy_available,
            common_proxy_id: None,
            account_proxy_required: inputs.gateway.account_proxy_required,
            quota_request_timeout_seconds: inputs.gateway.quota_request_timeout_seconds,
            chatgpt_interface_quota_reserve_basis_points: Some(
                inputs.gateway.chatgpt_interface_quota_reserve_basis_points,
            ),
            codex_background_tasks_enabled: inputs.gateway.codex_background_tasks_enabled,
            codex_websockets_enabled: inputs.gateway.codex_websockets_enabled,
            chatgpt_retry_until_available: inputs.gateway.chatgpt_retry_until_available,
            block_degraded_routes_enabled: inputs.gateway.block_degraded_routes_enabled,
            routing_order,
        },
        platform: platform::platform_name().to_string(),
        capabilities: Capabilities::desktop_local(),
        sources: source_summaries,
        accounts: account_summaries,
        automations: inputs.automations.tasks,
        wake_history: inputs.automations.state.history().iter().cloned().collect(),
        warnings,
        pricing: pricing_metadata,
    };
    state.record_performance_async(
        "full_snapshot_native",
        started.elapsed().as_secs_f64() * 1_000.0,
        Some("local"),
    );
    Ok(response)
}
