use super::{AccountRoutingBlockReason, OperationalStatus, ProxyMode, QuotaRefreshStatus};
use crate::{
    accounts::AccountAuthState,
    quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription},
    runtime_source_models_for_any_wire_api, runtime_source_models_for_wire_api,
    ApiEquivalentSummary, ApiModelPriceOverride, SourceProtocolBinding, WireApi,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

const MIN_WEEKLY_WINDOW_MINUTES: u32 = 6 * 24 * 60;
const MAX_WEEKLY_WINDOW_MINUTES: u32 = 8 * 24 * 60;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummary {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub operational_status: OperationalStatus,
    pub base_url: String,
    pub wire_api: WireApi,
    #[serde(default)]
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    #[serde(default)]
    pub recovery_delay_seconds: u64,
    #[serde(default)]
    pub model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
    /// Complete token prices discovered from this source's model catalog.
    /// Manual source overrides always take precedence.
    #[serde(default)]
    pub detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
    #[serde(default)]
    pub api_equivalent: ApiEquivalentSummary,
    pub secret_available: bool,
    pub last_error_code: Option<String>,
}

impl SourceSummary {
    /// Returns all models available through a client protocol. A source may
    /// expose more than one connector route for the same client protocol,
    /// such as native Responses and a Responses-to-Messages bridge.
    ///
    /// Legacy records without bindings retain their single `wire_api` surface.
    pub fn models_for_wire_api(&self, wire_api: WireApi) -> Vec<String> {
        runtime_source_models_for_wire_api(
            &self.protocol_bindings,
            self.wire_api,
            &self.models,
            wire_api,
        )
        .unwrap_or_default()
    }

    pub fn supports_wire_api(&self, wire_api: WireApi) -> bool {
        !self.models_for_wire_api(wire_api).is_empty()
    }

    /// Returns the union of models exposed by every confirmed source route.
    /// Native Gemini and Chat Completions sources must remain visible even
    /// though the desktop profile itself normally speaks Responses.
    pub fn models_for_any_wire_api(&self) -> Vec<String> {
        runtime_source_models_for_any_wire_api(&self.protocol_bindings, self.wire_api, &self.models)
            .unwrap_or_default()
    }

    pub fn supports_any_wire_api(&self) -> bool {
        !self.models_for_any_wire_api().is_empty()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccountLocation {
    pub server_id: String,
    pub remote_account_id: String,
}

/// API-price equivalent observed by Relay inside one provider quota window.
/// This is projection evidence, not an upstream balance or allowance.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindowUsage {
    pub kind: QuotaWindowKind,
    pub window_start_ms: u64,
    pub observed_at_ms: u64,
    pub window_minutes: u32,
    pub api_equivalent: ApiEquivalentSummary,
}

/// Chooses the provider's weekly window for a time-aligned value projection.
/// Short burst and monthly windows are deliberately excluded so the UI does
/// not present a different allowance as the weekly subscription value.
pub fn api_equivalent_projection_window(quota: &QuotaSnapshot) -> Option<&QuotaWindow> {
    quota
        .primary
        .iter()
        .chain(quota.secondary.iter())
        .filter(|window| {
            window.window_minutes.is_some_and(|minutes| {
                (MIN_WEEKLY_WINDOW_MINUTES..=MAX_WEEKLY_WINDOW_MINUTES).contains(&minutes)
            }) && window.window_start_ms.is_some()
                && window.available_basis_points.is_some()
                && window.observed_at_ms >= window.window_start_ms.unwrap_or_default()
        })
        .max_by_key(|window| window.window_minutes.unwrap_or_default())
}

#[cfg(test)]
mod quota_projection_tests {
    use super::*;

    fn window(kind: QuotaWindowKind, minutes: u32) -> QuotaWindow {
        QuotaWindow {
            kind,
            provider_cycle_id: None,
            window_start_ms: Some(1_000),
            available_basis_points: Some(5_000),
            explicitly_full: Some(false),
            reset_at_ms: Some(2_000),
            window_minutes: Some(minutes),
            observed_at_ms: 1_500,
            full_transition_fingerprint: None,
            exhaustion_transition_fingerprint: None,
        }
    }

    #[test]
    fn projection_uses_the_long_window_instead_of_the_burst_window() {
        let quota = QuotaSnapshot {
            primary: Some(window(QuotaWindowKind::Primary, 300)),
            secondary: Some(window(QuotaWindowKind::Secondary, 10_080)),
            ..Default::default()
        };

        assert_eq!(
            api_equivalent_projection_window(&quota).map(|window| window.kind),
            Some(QuotaWindowKind::Secondary)
        );
    }

    #[test]
    fn projection_is_absent_when_only_a_burst_window_exists() {
        let quota = QuotaSnapshot {
            primary: Some(window(QuotaWindowKind::Primary, 300)),
            ..Default::default()
        };

        assert!(api_equivalent_projection_window(&quota).is_none());
    }

    #[test]
    fn projection_does_not_substitute_a_monthly_window_for_the_weekly_window() {
        let quota = QuotaSnapshot {
            secondary: Some(window(QuotaWindowKind::Secondary, 30 * 24 * 60)),
            ..Default::default()
        };

        assert!(api_equivalent_projection_window(&quota).is_none());
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSummary {
    pub id: String,
    pub label: String,
    pub identity_hint: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    pub draining: bool,
    pub operational_status: OperationalStatus,
    pub auth_state: AccountAuthState,
    pub health: String,
    pub models: Vec<String>,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    #[serde(default)]
    pub api_equivalent: ApiEquivalentSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_window_usage: Option<QuotaWindowUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub purchase_cost_micro_usd: Option<u64>,
    pub subscription: Subscription,
    pub quota: QuotaSnapshot,
    #[serde(default)]
    pub quota_refresh_status: QuotaRefreshStatus,
    pub secret_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_location: Option<RemoteAccountLocation>,
    #[serde(default)]
    pub proxy_mode: ProxyMode,
    #[serde(default)]
    pub proxy_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing_block_reason: Option<AccountRoutingBlockReason>,
    pub last_error_code: Option<String>,
}

pub fn model_has_native_account_route(accounts: &[AccountSummary], model: &str) -> bool {
    accounts.iter().any(|account| {
        account.in_pool
            && account
                .models
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(model))
    })
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealedAccountIdentity {
    pub account_id: String,
    pub identity: String,
}

impl fmt::Debug for RevealedAccountIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RevealedAccountIdentity")
            .field("account_id", &self.account_id)
            .field("identity", &"[redacted]")
            .finish()
    }
}
