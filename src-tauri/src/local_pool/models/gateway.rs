use super::participant::validate_model_price_overrides;
use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use zenith_relay_core::{
    ApiModelPriceOverride, DefaultServiceTier, ModelContextPolicy,
    deserialize_model_reasoning_allowed_levels, normalize_model_context_policies,
    normalize_model_ids, normalize_model_reasoning_allowed_levels,
    normalize_model_service_tier_overrides,
};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindScope {
    Localhost,
    Lan,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettings {
    /// Explicit UI associations; each connection retains its own credential authority.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub account_routes: Vec<AccountRouteGroup>,
    #[serde(default)]
    pub tool_policy: zenith_relay_core::ToolPolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_routing: Option<zenith_relay_core::PoolRoutingPolicy>,
    pub enabled: bool,
    pub bind_scope: BindScope,
    pub port: u16,
    pub client_host: String,
    #[serde(default = "default_max_retry_candidates")]
    pub max_retry_candidates: u8,
    #[serde(default)]
    pub default_service_tier: DefaultServiceTier,
    #[serde(default)]
    pub image_base_model: Option<String>,
    #[serde(default)]
    pub common_proxy_configured: bool,
    #[serde(default)]
    pub account_proxy_required: bool,
    #[serde(default = "default_quota_request_timeout_seconds")]
    pub quota_request_timeout_seconds: u64,
    #[serde(default = "default_chatgpt_interface_quota_reserve_basis_points")]
    pub chatgpt_interface_quota_reserve_basis_points: u64,
    #[serde(default = "default_codex_background_tasks_enabled")]
    pub codex_background_tasks_enabled: bool,
    #[serde(default = "default_codex_background_tasks_enabled")]
    pub codex_memory_extraction_enabled: bool,
    #[serde(default = "default_codex_background_tasks_enabled")]
    pub codex_memory_consolidation_enabled: bool,
    #[serde(default = "default_codex_websockets_enabled")]
    pub codex_websockets_enabled: bool,
    /// Legacy persisted key for API text-route recovery, including non-ChatGPT
    /// clients. Retained so existing local settings survive upgrades.
    #[serde(default)]
    pub chatgpt_retry_until_available: bool,
    #[serde(default = "default_block_degraded_routes_enabled")]
    pub block_degraded_routes_enabled: bool,
    /// Explicit local opt-in; never adds saved accounts to the pool.
    #[serde(default)]
    pub codex_text_models_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_refresh_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog_refresh_error_at_ms: Option<u64>,
    #[serde(default)]
    pub hidden_models: Vec<String>,
    #[serde(default)]
    pub model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
    #[serde(
        default,
        alias = "modelReasoningOverrides",
        deserialize_with = "deserialize_model_reasoning_allowed_levels"
    )]
    pub model_reasoning_allowed_levels: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub model_service_tier_overrides: BTreeMap<String, DefaultServiceTier>,
    #[serde(default)]
    pub model_context_policies: BTreeMap<String, ModelContextPolicy>,
    #[serde(default)]
    pub model_display_order: Vec<String>,
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            account_routes: Vec::new(),
            tool_policy: Default::default(),
            enabled: false,
            bind_scope: BindScope::Localhost,
            port: DEFAULT_GATEWAY_PORT,
            client_host: "127.0.0.1".to_string(),
            max_retry_candidates: DEFAULT_MAX_RETRY_CANDIDATES,
            pool_routing: Some(zenith_relay_core::PoolRoutingPolicy::default()),
            default_service_tier: DefaultServiceTier::Standard,
            image_base_model: None,
            common_proxy_configured: false,
            account_proxy_required: false,
            quota_request_timeout_seconds: DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS,
            chatgpt_interface_quota_reserve_basis_points:
                DEFAULT_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS,
            codex_background_tasks_enabled: true,
            codex_memory_extraction_enabled: true,
            codex_memory_consolidation_enabled: true,
            codex_websockets_enabled: true,
            chatgpt_retry_until_available: false,
            block_degraded_routes_enabled: true,
            codex_text_models_enabled: false,
            catalog_refresh_error: None,
            catalog_refresh_error_at_ms: None,
            hidden_models: Vec::new(),
            model_price_overrides: BTreeMap::new(),
            model_reasoning_allowed_levels: BTreeMap::new(),
            model_service_tier_overrides: BTreeMap::new(),
            model_context_policies: BTreeMap::new(),
            model_display_order: Vec::new(),
        }
    }
}

impl GatewaySettings {
    pub fn account_route_group(&self, account_id: &str) -> Option<&AccountRouteGroup> {
        self.account_routes
            .iter()
            .find(|group| group.account_ids.iter().any(|id| id == account_id))
    }

    pub fn account_route_is_selected(&self, account_id: &str) -> bool {
        self.account_route_group(account_id)
            .is_none_or(|group| group.selected_account_id == account_id)
    }

    pub fn retain_account_routes(&mut self, accounts: &[LocalAccountRecord]) {
        if self.account_routes.is_empty() {
            return;
        }
        let ids = accounts
            .iter()
            .map(|account| account.account.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        self.account_routes
            .retain(|group| group.account_ids.iter().all(|id| ids.contains(id.as_str())));
    }

    pub fn pool_routing_for(
        &self,
        sources: &[ProviderSourceRecord],
        accounts: &[LocalAccountRecord],
    ) -> zenith_relay_core::PoolRoutingPolicy {
        zenith_relay_core::resolve_pool_routing(
            self.pool_routing.as_ref(),
            sources
                .iter()
                .filter(|m| m.in_pool)
                .map(|m| {
                    (
                        zenith_relay_core::PoolMemberKind::Source,
                        m.id.clone(),
                        m.priority,
                        m.weight,
                    )
                })
                .chain(
                    accounts
                        .iter()
                        .filter(|m| {
                            m.account.in_pool && self.account_route_is_selected(&m.account.id)
                        })
                        .map(|m| {
                            (
                                zenith_relay_core::PoolMemberKind::Account,
                                m.account.id.clone(),
                                m.priority,
                                m.weight,
                            )
                        }),
                )
                .collect(),
        )
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        zenith_relay_core::accounts::validate_account_route_groups(&self.account_routes)?;
        if self.port < 1024 {
            return Err("gateway port must be between 1024 and 65535");
        }
        if self.client_host != "127.0.0.1" && self.client_host != "localhost" {
            return Err("local gateway host must be localhost or 127.0.0.1");
        }
        if !zenith_relay_core::protocol::max_retry_candidates_in_range(self.max_retry_candidates) {
            return Err("max retry candidates must be between 1 and 8");
        }
        if let Some(policy) = &self.pool_routing {
            policy.validate()?;
        }
        if self
            .image_base_model
            .as_deref()
            .is_some_and(|model| model.len() > 256 || model.chars().any(char::is_control))
        {
            return Err("image base model id is invalid");
        }
        if !zenith_relay_core::protocol::quota_request_timeout_in_range(
            self.quota_request_timeout_seconds,
        ) {
            return Err("quota request timeout must be between 10 and 20 seconds");
        }
        if self.chatgpt_interface_quota_reserve_basis_points != 0
            && !(MIN_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS
                ..=MAX_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS)
                .contains(&self.chatgpt_interface_quota_reserve_basis_points)
        {
            return Err("ChatGPT account quota reserve must be disabled or between 1% and 99%");
        }
        validate_model_price_overrides(&self.model_price_overrides)?;
        normalize_model_reasoning_allowed_levels(self.model_reasoning_allowed_levels.clone())?;
        normalize_model_service_tier_overrides(self.model_service_tier_overrides.clone())?;
        normalize_model_context_policies(self.model_context_policies.clone())?;
        if normalize_model_ids(self.model_display_order.iter()) != self.model_display_order {
            return Err("model display order is invalid");
        }
        Ok(())
    }
}

fn default_quota_request_timeout_seconds() -> u64 {
    DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS
}

fn default_chatgpt_interface_quota_reserve_basis_points() -> u64 {
    DEFAULT_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS
}

fn default_codex_background_tasks_enabled() -> bool {
    true
}

fn default_block_degraded_routes_enabled() -> bool {
    true
}

fn default_codex_websockets_enabled() -> bool {
    true
}

fn default_max_retry_candidates() -> u8 {
    DEFAULT_MAX_RETRY_CANDIDATES
}
