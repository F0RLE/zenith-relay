use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zenith_relay_core::{
    accounts::AccountRecord,
    automations::{WakeAutomationState, WakeTask},
    normalize_model_price_overrides,
    protocol::RemoteAccountLocation,
    ApiModelPriceOverride, PoolAccess, PoolParticipant, SourceProtocolBinding,
    SourceProtocolConfig, SourceProtocolResolution, WireApi,
};

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderSourceRecord {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    #[serde(default)]
    pub in_pool: bool,
    #[serde(default)]
    pub draining: bool,
    pub base_url: String,
    pub secret_ref: String,
    /// Explicit LiteLLM namespace used for source pricing.  Legacy records
    /// leave this unset and therefore only use provider evidence/manual data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_provider: Option<String>,
    /// Opt-in canonical family fallback (for example `openai`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_provider_family: Option<String>,
    pub wire_api: WireApi,
    #[serde(default)]
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    #[serde(default)]
    pub protocol_config: SourceProtocolConfig,
    pub models: Vec<String>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub excluded_models: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default)]
    pub recovery_delay_seconds: u64,
    #[serde(default)]
    pub model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
    #[serde(default)]
    pub detected_model_prices: BTreeMap<String, ApiModelPriceOverride>,
    #[serde(default)]
    pub last_used_at: Option<String>,
    pub last_test_at: Option<String>,
    pub last_test_status: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalGatewayKeyRecord {
    pub id: String,
    pub label: String,
    pub enabled: bool,
    #[serde(default)]
    pub system: bool,
    pub secret_ref: String,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountRecord {
    pub account: AccountRecord,
    /// Explicit official pricing family. Older ChatGPT accounts default to
    /// `openai` in the resolver without requiring a migration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_family: Option<String>,
    #[serde(default)]
    pub purchase_cost_micro_usd: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_location: Option<RemoteAccountLocation>,
    pub wire_api: WireApi,
    pub models: Vec<String>,
    /// The last successful upstream discovery snapshot. `models` remains the
    /// imported/configured baseline and must not be replaced by background
    /// refreshes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discovered_models: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub excluded_models: Vec<String>,
    #[serde(default)]
    pub priority: i32,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default)]
    pub cooldowns: BTreeMap<String, u64>,
    #[serde(default)]
    pub consecutive_failures: u32,
    /// Observation from the official Codex client. This is informational and
    /// must never be used as a routing or account-switch hard block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_auth_status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_client_login_redirect_at_ms: Option<u64>,
}

impl LocalAccountRecord {
    /// Compares the operational record while ignoring Codex client-login
    /// observations. Those can change during a credential transaction and do
    /// not prove that this record was replaced.
    pub fn matches_rollback_snapshot(&self, attempted: &Self) -> bool {
        let mut comparable = self.clone();
        comparable.client_auth_status = attempted.client_auth_status.clone();
        comparable.last_client_login_redirect_at_ms = attempted.last_client_login_redirect_at_ms;
        comparable == *attempted
    }

    pub fn normalize(&mut self) {
        self.account.label = self.account.label.trim().to_string();
        if let Some(location) = &mut self.remote_location {
            location.server_id = location.server_id.trim().to_string();
            location.remote_account_id = location.remote_account_id.trim().to_string();
        }
        self.models = normalized_values(std::mem::take(&mut self.models));
        self.discovered_models = self.discovered_models.take().map(normalized_values);
        self.allowed_models = normalized_values(std::mem::take(&mut self.allowed_models));
        self.excluded_models = normalized_values(std::mem::take(&mut self.excluded_models));
        self.weight = self.weight.max(1);
        self.provider_family = self
            .provider_family
            .take()
            .map(|provider_name| provider_name.trim().to_ascii_lowercase())
            .filter(|provider_name| !provider_name.is_empty());
    }
}
zenith_relay_core::impl_effective_models!(LocalAccountRecord);

impl PoolParticipant for LocalAccountRecord {
    fn pool_access(&self) -> PoolAccess<'_> {
        PoolAccess {
            enabled: self.account.enabled,
            in_pool: self.account.in_pool,
            draining: self.account.draining,
            allowed_models: &self.allowed_models,
            excluded_models: &self.excluded_models,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRecords {
    pub tasks: Vec<WakeTask>,
    pub state: WakeAutomationState,
    #[serde(default)]
    pub weekly_reset_fingerprints: BTreeMap<String, String>,
}

impl Default for AutomationRecords {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            state: WakeAutomationState::new(1_024, 256)
                .expect("static wake automation bounds are valid"),
            weekly_reset_fingerprints: BTreeMap::new(),
        }
    }
}

#[cfg(test)]
pub(crate) fn synthetic_responses_source() -> ProviderSourceRecord {
    ProviderSourceRecord {
        id: "source".into(),
        name: "Provider".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        base_url: "https://provider.test/v1".into(),
        secret_ref: "source:test".into(),
        pricing_provider: None,
        official_provider_family: None,
        wire_api: WireApi::Responses,
        protocol_config: Default::default(),
        protocol_bindings: Vec::new(),
        models: vec!["model-a".into()],
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        recovery_delay_seconds: 0,
        model_price_overrides: BTreeMap::new(),
        detected_model_prices: BTreeMap::new(),
        last_used_at: None,
        last_test_at: None,
        last_test_status: None,
        last_error: None,
    }
}

impl ProviderSourceRecord {
    pub fn normalize(&mut self) {
        self.name = self.name.trim().to_string();
        self.base_url = self.base_url.trim().to_string();
        self.pricing_provider = self
            .pricing_provider
            .take()
            .map(|pricing_provider| pricing_provider.trim().to_ascii_lowercase())
            .filter(|pricing_provider| !pricing_provider.is_empty());
        self.official_provider_family = self
            .official_provider_family
            .take()
            .map(|provider_family| provider_family.trim().to_ascii_lowercase())
            .filter(|provider_family| !provider_family.is_empty());
        self.models = normalized_values(std::mem::take(&mut self.models));
        self.allowed_models = normalized_values(std::mem::take(&mut self.allowed_models));
        self.excluded_models = normalized_values(std::mem::take(&mut self.excluded_models));
        self.model_price_overrides = self
            .model_price_overrides
            .iter()
            .map(|(model, price)| (zenith_relay_core::model_id_key(model), *price))
            .collect();
        self.detected_model_prices = self
            .detected_model_prices
            .iter()
            .map(|(model, price)| (zenith_relay_core::model_id_key(model), *price))
            .collect();
        self.weight = self.weight.max(1);
    }

    pub fn validate_price_overrides(&self) -> Result<(), &'static str> {
        validate_model_price_overrides(&self.model_price_overrides)?;
        validate_model_price_overrides(&self.detected_model_prices)
    }

    /// Resolves the legacy single-protocol fields into the same shape used by
    /// current multi-protocol records without mutating persisted legacy data.
    pub fn effective_protocol_bindings(&self) -> Result<Vec<SourceProtocolBinding>, String> {
        SourceProtocolResolution::resolved_protocol_bindings(self)
    }

    pub fn validate_protocol_bindings(&self) -> Result<(), String> {
        self.effective_protocol_bindings().map(drop)
    }

    #[cfg(test)]
    pub fn models_for_wire_api(&self, wire_api: WireApi) -> Result<Vec<String>, String> {
        SourceProtocolResolution::resolved_models(self, Some(wire_api))
    }

    pub fn supports_any_wire_api(&self) -> Result<bool, String> {
        SourceProtocolResolution::resolved_supports_any(self)
    }
}

zenith_relay_core::impl_stored_source_record!(ProviderSourceRecord);

pub(super) fn validate_model_price_overrides(
    overrides: &BTreeMap<String, ApiModelPriceOverride>,
) -> Result<(), &'static str> {
    normalize_model_price_overrides(overrides.clone()).map(drop)
}

fn default_weight() -> u32 {
    1
}
