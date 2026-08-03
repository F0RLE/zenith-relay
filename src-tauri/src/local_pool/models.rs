use crate::platform::PlatformCapabilities;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use zenith_relay_core::{
    accounts::AccountRecord,
    automations::{WakeAutomationState, WakeHistory, WakeTask},
    normalize_model_price_overrides, normalize_source_protocol_bindings,
    normalize_subscription_plan_order,
    protocol::{ClientWireApi, RemoteAccountLocation},
    quota::QuotaEconomicsState,
    ApiModelPriceOverride, DefaultServiceTier, RoutingStrategy, SourceProtocolBinding, WireApi,
};

pub const CURRENT_SCHEMA_VERSION: u32 = 14;
pub const DEFAULT_GATEWAY_PORT: u16 = 14998;
pub const DEFAULT_MAX_RETRY_CANDIDATES: u8 = 3;
pub const DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;
pub const DEFAULT_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS: u64 = 100;
pub const MIN_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS: u64 = 100;
pub const MAX_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS: u64 = 9_900;
pub const MIN_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 10;
pub const MAX_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;
pub const MAX_LOCAL_ACCOUNTS: usize = 1_024;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BindScope {
    Localhost,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySettings {
    pub enabled: bool,
    pub bind_scope: BindScope,
    pub port: u16,
    pub client_host: String,
    #[serde(default = "default_max_retry_candidates")]
    pub max_retry_candidates: u8,
    #[serde(default)]
    pub routing_strategy: RoutingStrategy,
    #[serde(default)]
    pub subscription_plan_order: Vec<String>,
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
    #[serde(default)]
    pub hidden_models: Vec<String>,
    #[serde(default)]
    pub model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteTargetRecord {
    pub origin: String,
    pub server_id: String,
    pub identity_fingerprint: String,
    pub server_version: String,
    pub protocol_version: u16,
    pub allow_insecure_http: bool,
    pub secret_ref: String,
    pub connected_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipOperationKind {
    MoveToRemote,
    ReturnToLocal,
    ForceActivateLocal,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipOperationPhase {
    MovePrepared,
    MoveRemoteApplying,
    MoveRemoteCommitted,
    MoveLocalCommitted,
    ReturnPrepared,
    ReturnLocalStaged,
    ReturnRemoteRemoved,
    ReturnLocalCommitted,
    ForcePrepared,
    ForceLocalCommitted,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnershipOperationRecord {
    pub id: String,
    pub kind: OwnershipOperationKind,
    pub phase: OwnershipOperationPhase,
    pub server_id: String,
    pub local_account_ids: Vec<String>,
    #[serde(default)]
    pub remote_account_ids: Vec<String>,
    #[serde(default)]
    pub created_remote_account_ids: Vec<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl OwnershipOperationRecord {
    pub fn validate(&self) -> Result<(), &'static str> {
        let valid_id = |value: &str, prefix: &str| {
            value.strip_prefix(prefix).is_some_and(|suffix| {
                suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        };
        let valid_object_id = |value: &str| {
            !value.is_empty()
                && value.len() <= 128
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        };
        if !valid_id(&self.id, "ownership_")
            || self.server_id.is_empty()
            || self.server_id.len() > 128
            || self.local_account_ids.is_empty()
            || self.local_account_ids.len() > 256
            || self.local_account_ids.iter().any(|id| !valid_object_id(id))
            || self
                .remote_account_ids
                .iter()
                .any(|id| !valid_object_id(id))
            || self
                .created_remote_account_ids
                .iter()
                .any(|id| !valid_object_id(id))
            || self.updated_at_ms < self.created_at_ms
        {
            return Err("remote ownership operation is invalid");
        }
        let mut local_ids = HashSet::new();
        let mut remote_ids = HashSet::new();
        let mut created_ids = HashSet::new();
        if self
            .local_account_ids
            .iter()
            .any(|id| !local_ids.insert(id))
            || self
                .remote_account_ids
                .iter()
                .any(|id| !remote_ids.insert(id))
            || self
                .created_remote_account_ids
                .iter()
                .any(|id| !created_ids.insert(id))
            || self.remote_account_ids.len() > self.local_account_ids.len()
            || self.created_remote_account_ids.len() > self.local_account_ids.len()
        {
            return Err("remote ownership operation contains inconsistent account ids");
        }
        let valid_phase = matches!(
            (self.kind, self.phase),
            (
                OwnershipOperationKind::MoveToRemote,
                OwnershipOperationPhase::MovePrepared
                    | OwnershipOperationPhase::MoveRemoteApplying
                    | OwnershipOperationPhase::MoveRemoteCommitted
                    | OwnershipOperationPhase::MoveLocalCommitted
            ) | (
                OwnershipOperationKind::ReturnToLocal,
                OwnershipOperationPhase::ReturnPrepared
                    | OwnershipOperationPhase::ReturnLocalStaged
                    | OwnershipOperationPhase::ReturnRemoteRemoved
                    | OwnershipOperationPhase::ReturnLocalCommitted
            ) | (
                OwnershipOperationKind::ForceActivateLocal,
                OwnershipOperationPhase::ForcePrepared
                    | OwnershipOperationPhase::ForceLocalCommitted
            )
        );
        if !valid_phase {
            return Err("remote ownership operation phase is invalid");
        }
        Ok(())
    }
}

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
    pub wire_api: WireApi,
    #[serde(default)]
    pub protocol_bindings: Vec<SourceProtocolBinding>,
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
    #[serde(default)]
    pub source_ids: Option<Vec<String>>,
    #[serde(default)]
    pub account_ids: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_models: Vec<String>,
    #[serde(default)]
    pub excluded_models: Vec<String>,
    #[serde(default)]
    pub model_prefix: Option<String>,
    /// `None` preserves a legacy unrestricted protocol scope. New keys always
    /// persist an explicit non-empty set, so a client key cannot accidentally
    /// expose a native endpoint the user did not choose.
    #[serde(default)]
    pub wire_apis: Option<Vec<ClientWireApi>>,
    pub created_at: String,
    pub last_used_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountRecord {
    pub account: AccountRecord,
    #[serde(default)]
    pub economics: QuotaEconomicsState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_location: Option<RemoteAccountLocation>,
    pub wire_api: WireApi,
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
    pub cooldowns: BTreeMap<String, u64>,
    #[serde(default)]
    pub consecutive_failures: u32,
}

impl LocalAccountRecord {
    pub fn normalize(&mut self) {
        self.account.label = self.account.label.trim().to_string();
        if let Some(location) = &mut self.remote_location {
            location.server_id = location.server_id.trim().to_string();
            location.remote_account_id = location.remote_account_id.trim().to_string();
        }
        self.models = normalized_values(std::mem::take(&mut self.models));
        self.allowed_models = normalized_values(std::mem::take(&mut self.allowed_models));
        self.excluded_models = normalized_values(std::mem::take(&mut self.excluded_models));
        self.weight = self.weight.max(1);
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutomationRecords {
    pub tasks: Vec<WakeTask>,
    pub state: WakeAutomationState,
}

impl Default for AutomationRecords {
    fn default() -> Self {
        Self {
            tasks: Vec::new(),
            state: WakeAutomationState::new(1_024, 256)
                .expect("static wake automation bounds are valid"),
        }
    }
}

impl Default for GatewaySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_scope: BindScope::Localhost,
            port: DEFAULT_GATEWAY_PORT,
            client_host: "127.0.0.1".to_string(),
            max_retry_candidates: DEFAULT_MAX_RETRY_CANDIDATES,
            routing_strategy: RoutingStrategy::Adaptive,
            subscription_plan_order: Vec::new(),
            default_service_tier: DefaultServiceTier::Standard,
            image_base_model: None,
            common_proxy_configured: false,
            account_proxy_required: false,
            quota_request_timeout_seconds: DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS,
            chatgpt_interface_quota_reserve_basis_points:
                DEFAULT_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS,
            hidden_models: Vec::new(),
            model_price_overrides: BTreeMap::new(),
        }
    }
}

impl GatewaySettings {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.port < 1024 {
            return Err("gateway port must be between 1024 and 65535");
        }
        if self.client_host != "127.0.0.1" && self.client_host != "localhost" {
            return Err("local gateway host must be localhost or 127.0.0.1");
        }
        if !(1..=8).contains(&self.max_retry_candidates) {
            return Err("max retry candidates must be between 1 and 8");
        }
        normalize_subscription_plan_order(self.subscription_plan_order.clone())?;
        if self
            .image_base_model
            .as_deref()
            .is_some_and(|model| model.len() > 256 || model.chars().any(char::is_control))
        {
            return Err("image base model id is invalid");
        }
        if !(MIN_QUOTA_REQUEST_TIMEOUT_SECONDS..=MAX_QUOTA_REQUEST_TIMEOUT_SECONDS)
            .contains(&self.quota_request_timeout_seconds)
        {
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
        Ok(())
    }
}

fn default_quota_request_timeout_seconds() -> u64 {
    DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS
}

fn default_chatgpt_interface_quota_reserve_basis_points() -> u64 {
    DEFAULT_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS
}

impl ProviderSourceRecord {
    pub fn normalize(&mut self) {
        self.name = self.name.trim().to_string();
        self.base_url = self.base_url.trim().to_string();
        self.models = normalized_values(std::mem::take(&mut self.models));
        self.allowed_models = normalized_values(std::mem::take(&mut self.allowed_models));
        self.excluded_models = normalized_values(std::mem::take(&mut self.excluded_models));
        self.model_price_overrides = self
            .model_price_overrides
            .iter()
            .map(|(model, price)| (model.trim().to_ascii_lowercase(), *price))
            .collect();
        self.weight = self.weight.max(1);
    }

    pub fn validate_price_overrides(&self) -> Result<(), &'static str> {
        validate_model_price_overrides(&self.model_price_overrides)
    }

    /// Resolves the legacy single-protocol fields into the same shape used by
    /// current multi-protocol records without mutating persisted legacy data.
    pub fn effective_protocol_bindings(&self) -> Result<Vec<SourceProtocolBinding>, String> {
        normalize_source_protocol_bindings(
            self.protocol_bindings.clone(),
            self.wire_api,
            &self.models,
        )
        .map_err(|error| error.to_string())
    }

    pub fn normalize_protocol_bindings(&mut self) -> Result<(), String> {
        if self.protocol_bindings.is_empty() {
            return Ok(());
        }
        let bindings = self.effective_protocol_bindings()?;
        // `wire_api` is a compatibility default for legacy readers. Do not
        // derive it from route order: an explicit mixed source is defined by
        // `protocol_bindings`, and discovery is free to return routes in any
        // stable upstream order.
        self.protocol_bindings = bindings;
        Ok(())
    }

    pub fn validate_protocol_bindings(&self) -> Result<(), String> {
        self.effective_protocol_bindings().map(drop)
    }

    pub fn models_for_wire_api(&self, wire_api: WireApi) -> Result<Vec<String>, String> {
        let mut seen = HashSet::new();
        let mut models = Vec::new();
        for binding in self
            .effective_protocol_bindings()?
            .into_iter()
            .filter(|binding| binding.wire_api == wire_api)
        {
            for model in binding.model_ids {
                if seen.insert(model.to_ascii_lowercase()) {
                    models.push(model);
                }
            }
        }
        Ok(models)
    }

    pub fn supports_wire_api(&self, wire_api: WireApi) -> Result<bool, String> {
        Ok(!self.models_for_wire_api(wire_api)?.is_empty())
    }
}

fn validate_model_price_overrides(
    overrides: &BTreeMap<String, ApiModelPriceOverride>,
) -> Result<(), &'static str> {
    normalize_model_price_overrides(overrides.clone()).map(drop)
}

impl LocalGatewayKeyRecord {
    pub fn normalize(&mut self) {
        self.label = self.label.trim().to_string();
        self.source_ids = self.source_ids.take().map(normalized_values);
        self.account_ids = self.account_ids.take().map(normalized_values);
        self.allowed_models = normalized_values(std::mem::take(&mut self.allowed_models));
        self.excluded_models = normalized_values(std::mem::take(&mut self.excluded_models));
        self.model_prefix = self
            .model_prefix
            .take()
            .map(|prefix| prefix.trim().trim_matches('/').to_string())
            .filter(|prefix| !prefix.is_empty());
        self.wire_apis = self.wire_apis.take().map(|wire_apis| {
            wire_apis
                .into_iter()
                .map(normalize_client_wire_api)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect()
        });
    }
}

fn normalize_client_wire_api(wire_api: ClientWireApi) -> ClientWireApi {
    // Images are served through the Chat Completions-compatible surface. Keep
    // old persisted values readable but never retain a misleading standalone
    // image protocol scope.
    match wire_api {
        ClientWireApi::Images => ClientWireApi::ChatCompletions,
        other => other,
    }
}

pub(crate) fn normalized_values(values: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .filter(|value| seen.insert(value.to_ascii_lowercase()))
        .collect()
}

fn default_max_retry_candidates() -> u8 {
    DEFAULT_MAX_RETRY_CANDIDATES
}

fn default_weight() -> u32 {
    1
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTarget {
    pub kind: &'static str,
    pub connected: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalPoolSnapshot {
    pub schema_version: u32,
    pub runtime_target: RuntimeTarget,
    pub gateway: GatewaySettings,
    pub platform: &'static str,
    pub capabilities: PlatformCapabilities,
    pub sources: Vec<ProviderSourceRecord>,
    pub accounts: Vec<LocalAccountRecord>,
    pub keys: Vec<LocalGatewayKeyRecord>,
    pub automations: Vec<WakeTask>,
    pub wake_history: Vec<WakeHistory>,
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_validation_rejects_privileged_port_and_remote_host() {
        let mut settings = GatewaySettings {
            port: 443,
            ..GatewaySettings::default()
        };
        assert!(settings.validate().is_err());

        settings.port = DEFAULT_GATEWAY_PORT;
        settings.client_host = "0.0.0.0".to_string();
        assert!(settings.validate().is_err());

        settings.client_host = "127.0.0.1".to_string();
        settings.max_retry_candidates = 0;
        assert!(settings.validate().is_err());
    }

    #[test]
    fn gateway_validation_bounds_quota_policy() {
        let mut settings = GatewaySettings::default();
        assert!(settings.validate().is_ok());
        settings.quota_request_timeout_seconds = MIN_QUOTA_REQUEST_TIMEOUT_SECONDS - 1;
        assert!(settings.validate().is_err());
        settings.quota_request_timeout_seconds = MIN_QUOTA_REQUEST_TIMEOUT_SECONDS;
        assert!(settings.validate().is_ok());

        settings.chatgpt_interface_quota_reserve_basis_points = 0;
        assert!(settings.validate().is_ok());
        settings.chatgpt_interface_quota_reserve_basis_points = 99;
        assert!(settings.validate().is_err());
        settings.chatgpt_interface_quota_reserve_basis_points = 100;
        assert!(settings.validate().is_ok());
        settings.chatgpt_interface_quota_reserve_basis_points =
            MAX_CHATGPT_INTERFACE_QUOTA_RESERVE_BASIS_POINTS;
        assert!(settings.validate().is_ok());
    }

    #[test]
    fn record_normalization_preserves_explicit_empty_key_scope() {
        let mut key = LocalGatewayKeyRecord {
            id: "key_1".into(),
            label: "  Scoped  ".into(),
            enabled: true,
            system: false,
            secret_ref: "key:key_1".into(),
            source_ids: Some(vec![" source_1 ".into(), "SOURCE_1".into()]),
            account_ids: Some(vec![" account_1 ".into(), "ACCOUNT_1".into()]),
            allowed_models: vec![" gpt-test ".into()],
            excluded_models: Vec::new(),
            model_prefix: Some(" /team/ ".into()),
            wire_apis: Some(vec![
                ClientWireApi::Images,
                ClientWireApi::ChatCompletions,
                ClientWireApi::Messages,
            ]),
            created_at: "2026-07-10T00:00:00Z".into(),
            last_used_at: None,
        };
        key.normalize();
        assert_eq!(key.label, "Scoped");
        assert_eq!(key.source_ids, Some(vec!["source_1".into()]));
        assert_eq!(key.account_ids, Some(vec!["account_1".into()]));
        assert_eq!(key.model_prefix.as_deref(), Some("team"));
        assert_eq!(
            key.wire_apis,
            Some(vec![
                ClientWireApi::ChatCompletions,
                ClientWireApi::Messages
            ])
        );

        key.source_ids = Some(Vec::new());
        key.account_ids = Some(Vec::new());
        key.normalize();
        assert_eq!(key.source_ids, Some(Vec::new()));
        assert_eq!(key.account_ids, Some(Vec::new()));
    }
}
