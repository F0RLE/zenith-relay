use crate::local_pool::{error::CommandError, state::DesktopState};
#[cfg(test)]
use std::collections::BTreeMap;
use tauri::State;
use zenith_relay_core::protocol::RuntimeStateSnapshot;
#[cfg(test)]
use zenith_relay_core::protocol::{DesktopCapabilities, GatewaySummary};
mod snapshot;

#[cfg(test)]
use snapshot::oauth_account_runtime_available;
pub(crate) use snapshot::{build_local_runtime_state, build_streaming_client_state};

use zenith_relay_core::CandidateRuntimeSnapshot;
#[cfg(test)]
use zenith_relay_core::{CandidateKind, PricingMetadata};

#[tauri::command]
pub async fn get_local_runtime_state(
    state: State<'_, DesktopState>,
) -> Result<RuntimeStateSnapshot, CommandError> {
    build_local_runtime_state(&state).await
}

#[tauri::command]
pub async fn record_local_performance_sample(
    name: String,
    duration_ms: f64,
    context: Option<String>,
    state: State<'_, DesktopState>,
) -> Result<(), CommandError> {
    let session = crate::diagnostics::DebugSession::capture();
    state
        .telemetry
        .record_performance_async(name, duration_ms, context, move || {
            session.is_some_and(crate::diagnostics::DebugSession::is_current)
        })
        .await
        .map_err(Into::into)
}

#[tauri::command]
pub async fn get_local_runtime_order(
    state: State<'_, DesktopState>,
) -> Result<RuntimeRoutingView, CommandError> {
    let routing_order = state
        .gateway
        .runtime()
        .await
        .map(|runtime| runtime.candidate_runtime_order_for_key(super::pool::SYSTEM_GATEWAY_KEY_ID))
        .unwrap_or_default();
    let pool_rotation = state
        .pool_rotation_projection
        .lock()
        .map_err(|_| {
            crate::local_pool::error::LocalPoolError::invalid_state(
                "pool rotation projection lock poisoned",
            )
        })?
        .as_ref()
        .map(|(revision, projection)| {
            let mut view = projection.view(&routing_order, zenith_relay_core::unix_time_ms());
            view.inventory_revision = *revision;
            view
        });
    Ok(RuntimeRoutingView {
        routing_order,
        pool_rotation,
    })
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRoutingView {
    routing_order: Vec<CandidateRuntimeSnapshot>,
    pool_rotation: Option<zenith_relay_core::protocol::PoolRotationView>,
}

#[cfg(test)]
mod parity_tests {
    use super::*;

    #[test]
    fn local_snapshot_omits_retired_fields_and_accepts_legacy_transport_flag() {
        let local = serde_json::to_value(RuntimeStateSnapshot {
            schema_version: 1,
            weekly_capacity_revision: 0,
            gateway: GatewaySummary {
                tool_policy: Default::default(),
                pool_routing: None,
                pool_rotation: Default::default(),
                running: false,
                base_url: "http://127.0.0.1:14998/v1".into(),
                candidate_count: 0,
                visible_model_ids: Vec::new(),
                max_retry_candidates: 3,
                default_service_tier: Default::default(),
                models: Vec::new(),
                model_catalog: BTreeMap::new(),
                common_proxy_configured: false,
                common_proxy_available: false,
                common_proxy_id: None,
                account_proxy_required: false,
                quota_request_timeout_seconds: 20,
                chatgpt_interface_quota_reserve_basis_points: Some(100),
                codex_background_tasks_enabled: true,
                codex_memory_extraction_enabled: true,
                codex_memory_consolidation_enabled: true,
                codex_websockets_enabled: true,
                chatgpt_retry_until_available: false,
                block_degraded_routes_enabled: true,
                codex_text_models_enabled: false,
                routing_order: Vec::new(),
            },
            platform: "test".into(),
            capabilities: DesktopCapabilities::current(),
            sources: Vec::new(),
            accounts: Vec::new(),
            automations: Vec::new(),
            wake_history: Vec::new(),
            warnings: Vec::new(),
            pricing: PricingMetadata::default(),
        })
        .unwrap();
        assert!(local.get("runtimeTarget").is_none());
        assert!(local.get("configurationRevision").is_none());
        assert!(local["gateway"].get("basisPointsEnabled").is_none());
        for legacy_enabled in [false, true] {
            let mut legacy_gateway = local["gateway"].clone();
            legacy_gateway["basisPointsEnabled"] = legacy_enabled.into();
            let gateway: GatewaySummary = serde_json::from_value(legacy_gateway).unwrap();
            assert_eq!(serde_json::to_value(gateway).unwrap(), local["gateway"]);
        }
        let mut capability_fields = local["capabilities"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        capability_fields.sort_unstable();
        assert_eq!(capability_fields, ["accountConnectors", "features"]);
        assert!(
            !local["capabilities"]["accountConnectors"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn runtime_account_presence_is_bound_to_the_oauth_candidate() {
        let candidate = CandidateRuntimeSnapshot {
            candidate_id: "account_plus".into(),
            kind: CandidateKind::OAuthAccount,
            available: true,
            quota_wait: false,
            capacity_wait: false,
            next_for_new_request: false,
            selection_rank: None,
            resource_headroom: None,
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
            quota_wait: false,
            capacity_wait: false,
            next_for_new_request: false,
            selection_rank: None,
            resource_headroom: None,
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
