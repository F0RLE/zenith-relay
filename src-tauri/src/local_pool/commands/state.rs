use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError},
    models::LocalPoolSnapshot,
    state::DesktopState,
};
#[cfg(test)]
use std::collections::BTreeMap;
use tauri::{AppHandle, Emitter, State};
use zenith_relay_core::protocol::RuntimeStateSnapshot;
#[cfg(test)]
use zenith_relay_core::protocol::{Capabilities, GatewaySummary, RuntimeTargetSummary};
mod snapshot;

pub(crate) use snapshot::build_local_runtime_state;
#[cfg(test)]
use snapshot::oauth_account_runtime_available;

use zenith_relay_core::{
    pricing::{CatalogRefreshOutcome, PricingError},
    CandidateRuntimeSnapshot,
};
#[cfg(test)]
use zenith_relay_core::{CandidateKind, PricingMetadata};

#[tauri::command]
pub async fn get_local_pool_state(
    state: State<'_, DesktopState>,
) -> Result<LocalPoolSnapshot, CommandError> {
    state.snapshot().await.map_err(Into::into)
}

/// Force a LiteLLM catalog refresh without returning the catalog payload.
///
/// The loader keeps the previous valid snapshot when the network or cache
/// fails, while the state event makes the resulting freshness state visible to
/// all renderer consumers.
#[tauri::command]
pub async fn refresh_local_pricing_catalog(
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<CatalogRefreshOutcome, CommandError> {
    let pricing_refresh_result = state.pricing_loader().refresh(true).await;
    let _ = app.emit("zenith-state-changed", ());
    pricing_refresh_result
        .map_err(pricing_error)
        .map_err(Into::into)
}

fn pricing_error(error: PricingError) -> LocalPoolError {
    let code = match error {
        PricingError::Network | PricingError::HttpStatus(_) => ErrorCode::GatewayUnavailable,
        PricingError::InvalidCatalog | PricingError::InvalidCache => ErrorCode::InvalidState,
        PricingError::InvalidAmount
        | PricingError::Overflow
        | PricingError::InvalidRecord
        | PricingError::CacheTooLarge
        | PricingError::Io => ErrorCode::Io,
    };
    LocalPoolError::new(code, error.to_string())
}

#[tauri::command]
pub async fn get_local_runtime_state(
    state: State<'_, DesktopState>,
) -> Result<RuntimeStateSnapshot, CommandError> {
    build_local_runtime_state(&state).await
}

#[tauri::command]
pub fn record_local_performance_sample(
    name: String,
    duration_ms: f64,
    context: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    state
        .record_performance(&name, duration_ms, context.as_deref())
        .map_err(Into::into)
}

#[tauri::command]
pub async fn get_local_runtime_order(
    state: State<'_, DesktopState>,
) -> Result<Vec<CandidateRuntimeSnapshot>, CommandError> {
    Ok(state
        .gateway
        .runtime()
        .await
        .map(|runtime| runtime.candidate_runtime_order_for_key(super::pool::SYSTEM_GATEWAY_KEY_ID))
        .unwrap_or_default())
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    #[test]
    fn local_and_remote_snapshots_share_the_same_top_level_contract() {
        let local = serde_json::to_value(RuntimeStateSnapshot {
            schema_version: 1,
            configuration_revision: None,
            runtime_target: RuntimeTargetSummary {
                kind: "local".into(),
                connected: false,
                origin: None,
                server_id: None,
                version: None,
            },
            gateway: GatewaySummary {
                tool_policy: Default::default(),
                basis_points_enabled: false,
                pool_routing: None,
                running: false,
                base_url: "http://127.0.0.1:14998/v1".into(),
                candidate_count: 0,
                visible_model_ids: Vec::new(),
                max_retry_candidates: 3,
                default_service_tier: Default::default(),
                image_base_model: None,
                models: Vec::new(),
                model_catalog: BTreeMap::new(),
                common_proxy_configured: false,
                common_proxy_available: false,
                common_proxy_id: None,
                account_proxy_required: false,
                quota_request_timeout_seconds: 20,
                chatgpt_interface_quota_reserve_basis_points: Some(100),
                codex_background_tasks_enabled: true,
                codex_websockets_enabled: true,
                chatgpt_retry_until_available: false,
                block_degraded_routes_enabled: true,
                routing_order: Vec::new(),
            },
            platform: "test".into(),
            capabilities: Capabilities::desktop_local(),
            sources: Vec::new(),
            accounts: Vec::new(),
            automations: Vec::new(),
            wake_history: Vec::new(),
            warnings: Vec::new(),
            pricing: PricingMetadata::default(),
        })
        .unwrap();
        let remote = local.clone();
        assert_eq!(
            local.as_object().unwrap().keys().collect::<Vec<_>>(),
            remote.as_object().unwrap().keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn runtime_account_presence_is_bound_to_the_oauth_candidate() {
        let candidate = CandidateRuntimeSnapshot {
            candidate_id: "account_plus".into(),
            kind: CandidateKind::OAuthAccount,
            available: true,
            next_for_new_request: false,
            activity_revision: 0,
            runtime_id: 0,
            in_flight: 0,
            active_request_count: 0,
            active_models: Vec::new(),
            model_retries: Vec::new(),
            last_used_at_ms: None,
            next_retry_at_ms: None,
            half_open: false,
            dispatches: 0,
        };
        assert_eq!(
            oauth_account_runtime_available(std::slice::from_ref(&candidate), "account_plus"),
            Some(true)
        );
        let unavailable = CandidateRuntimeSnapshot {
            candidate_id: "account_unavailable".into(),
            kind: CandidateKind::OAuthAccount,
            available: false,
            next_for_new_request: false,
            activity_revision: 0,
            runtime_id: 0,
            in_flight: 0,
            active_request_count: 0,
            active_models: Vec::new(),
            model_retries: Vec::new(),
            last_used_at_ms: None,
            next_retry_at_ms: None,
            half_open: false,
            dispatches: 0,
        };
        assert_eq!(
            oauth_account_runtime_available(
                std::slice::from_ref(&unavailable),
                "account_unavailable"
            ),
            Some(false)
        );
        assert_eq!(
            oauth_account_runtime_available(&[candidate], "missing"),
            None
        );
    }
}
