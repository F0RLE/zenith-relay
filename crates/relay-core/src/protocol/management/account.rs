use super::{AccountRoutingBlockReason, OperationalStatus, ProxyMode, QuotaRefreshStatus};
use crate::{
    accounts::AccountAuthState,
    quota::{QuotaSnapshot, QuotaWindowKind, Subscription},
    runtime_source_models_for_wire_api, ApiEquivalentSummary, ApiModelPriceOverride,
    SourceProtocolBinding, WireApi,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fmt};

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
        let mut seen = std::collections::BTreeSet::new();
        WireApi::ALL
            .into_iter()
            .flat_map(|wire_api| self.models_for_wire_api(wire_api))
            .filter(|model| seen.insert(model.to_ascii_lowercase()))
            .collect()
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaWindowUsage {
    pub kind: QuotaWindowKind,
    pub window_start_ms: u64,
    pub api_equivalent: ApiEquivalentSummary,
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
