use super::{OperationalStatus, RefreshStatus};
use crate::{
    ApiEquivalentSummary, ApiModelPriceOverride, SourceProtocolBinding, SourceProtocolConfig,
    SourceProtocolResolution, WireApi,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRefreshState {
    pub models: RefreshStatus,
    pub balance: RefreshStatus,
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_provider_family: Option<String>,
    pub wire_api: WireApi,
    #[serde(default)]
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    #[serde(default)]
    pub protocol_config: SourceProtocolConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_protocol_bindings: Option<Vec<SourceProtocolBinding>>,
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
    /// Non-secret source incarnation/configuration revision for UI observation scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_revision: Option<u64>,
    /// Independent model and statistics evidence; neither controls routing.
    #[serde(default)]
    pub refresh_state: SourceRefreshState,
    /// Runtime-only cached observation. Snapshot reads never contact a provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_stats: Option<crate::SourceProviderStats>,
}

impl SourceSummary {
    /// Returns all models available through a client protocol. A source may
    /// expose more than one connector route for the same client protocol,
    /// such as native Responses and a Responses-to-Messages bridge.
    ///
    /// Persisted legacy bindings do not restrict the automatic client surface.
    /// Relay selects a native upstream when available and otherwise adapts.
    pub fn models_for_wire_api(&self, wire_api: WireApi) -> Vec<String> {
        SourceProtocolResolution::resolved_models(self, Some(wire_api)).unwrap_or_default()
    }

    pub fn supports_wire_api(&self, wire_api: WireApi) -> bool {
        !self.models_for_wire_api(wire_api).is_empty()
    }

    /// Returns the union of models exposed by every confirmed source route.
    /// Native Gemini and Chat Completions sources must remain visible even
    /// though the desktop profile itself normally speaks Responses.
    pub fn models_for_any_wire_api(&self) -> Vec<String> {
        SourceProtocolResolution::resolved_models(self, None).unwrap_or_default()
    }

    pub fn supports_any_wire_api(&self) -> bool {
        !self.models_for_any_wire_api().is_empty()
    }

    /// Returns models that have at least one confirmed Anthropic-style
    /// Messages upstream route. Cache creation tariffs are valid only for
    /// those routes, even when the same model is also exposed by Responses or
    /// another generic API route.
    pub fn models_with_cache_write_pricing(&self) -> BTreeSet<String> {
        crate::cache_write_model_ids(
            SourceProtocolResolution::resolved_protocol_bindings(self).unwrap_or_default(),
        )
    }
}

impl SourceProtocolResolution for SourceSummary {
    fn protocol_base_url(&self) -> &str {
        &self.base_url
    }

    fn protocol_models(&self) -> &[String] {
        &self.models
    }

    fn stored_protocol_bindings(&self) -> &[SourceProtocolBinding] {
        &self.protocol_bindings
    }

    fn protocol_fallback(&self) -> WireApi {
        self.wire_api
    }

    fn source_protocol_config(&self) -> &SourceProtocolConfig {
        &self.protocol_config
    }
}
