use super::Capabilities;
use crate::{
    account_candidate_health,
    accounts::{AccountAuthState, AccountHealthState},
    api_model_price,
    automations::{WakeHistory, WakeTask},
    codex_model_display_name, codex_model_is_picker_eligible,
    quota::{QuotaSnapshot, Subscription, SubscriptionStatus},
    ApiEquivalentSummary, ApiModelPriceOverride, CandidateHealth, CandidateKind, CandidateQuota,
    CandidateRuntimeSnapshot, DefaultServiceTier, ModelRules, RoutingDiagnostics, RoutingStrategy,
    SourceProtocolBinding, WireApi,
};
use serde::{ser::SerializeStruct, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub server_id: String,
    pub started_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeTargetSummary {
    pub kind: String,
    pub connected: bool,
    pub origin: Option<String>,
    pub server_id: Option<String>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewaySummary {
    pub running: bool,
    pub base_url: String,
    pub candidate_count: usize,
    pub visible_model_ids: Vec<String>,
    pub max_retry_candidates: u8,
    #[serde(default = "default_cooldown_after_failures")]
    pub cooldown_after_failures: u8,
    #[serde(default = "default_keep_last_candidate_available")]
    pub keep_last_candidate_available: bool,
    pub routing_strategy: RoutingStrategy,
    #[serde(default)]
    pub subscription_plan_order: Vec<String>,
    pub default_service_tier: DefaultServiceTier,
    #[serde(default)]
    pub image_base_model: Option<String>,
    #[serde(default)]
    pub models: Vec<ModelSummary>,
    #[serde(default)]
    pub common_proxy_configured: bool,
    #[serde(default)]
    pub common_proxy_available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub common_proxy_id: Option<String>,
    #[serde(default)]
    pub account_proxy_required: bool,
    #[serde(default)]
    pub quota_request_timeout_seconds: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_interface_quota_reserve_basis_points: Option<u64>,
    #[serde(default)]
    pub routing_order: Vec<CandidateRuntimeSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSummary {
    pub id: String,
    pub enabled: bool,
    pub member_count: usize,
    #[serde(default)]
    pub codex_visible: bool,
    #[serde(default)]
    pub codex_display_name: String,
    pub catalog_rank: Option<u32>,
    pub input_micro_usd_per_million: Option<u64>,
    pub cached_input_micro_usd_per_million: Option<u64>,
    #[serde(default)]
    pub cache_write_5m_micro_usd_per_million: Option<u64>,
    #[serde(default)]
    pub cache_write_1h_micro_usd_per_million: Option<u64>,
    pub output_micro_usd_per_million: Option<u64>,
    #[serde(default)]
    pub custom_price: bool,
    #[serde(default)]
    pub reasoning_levels: Vec<String>,
    #[serde(default)]
    pub reasoning_allowed_levels: Vec<String>,
    #[serde(default)]
    pub reasoning_configurable: bool,
}

/// Adds the confirmed API-source reasoning capabilities to a management
/// model row. Native ChatGPT routes retain their provider-owned catalog and
/// are intentionally never configurable here.
pub fn apply_model_reasoning_summary(
    model: &mut ModelSummary,
    confirmed_levels: Vec<String>,
    saved_allowed_levels: Option<&[String]>,
    has_native_account_route: bool,
) {
    model.reasoning_levels.clear();
    model.reasoning_allowed_levels.clear();
    model.reasoning_configurable = false;
    if has_native_account_route {
        return;
    }

    let mut seen = BTreeSet::new();
    for level in confirmed_levels {
        let level = level.trim().to_ascii_lowercase();
        if !level.is_empty() && seen.insert(level.clone()) {
            model.reasoning_levels.push(level);
        }
    }
    if model.reasoning_levels.is_empty() {
        return;
    }

    model.reasoning_configurable = true;
    if let Some(saved_allowed_levels) = saved_allowed_levels {
        let configured = saved_allowed_levels
            .iter()
            .map(|level| level.trim().to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        model.reasoning_allowed_levels = model
            .reasoning_levels
            .iter()
            .filter(|level| configured.contains(level.as_str()))
            .cloned()
            .collect();
    }
}

pub fn source_runtime_available(
    routing_order: &[CandidateRuntimeSnapshot],
    source_id: &str,
) -> bool {
    routing_order.iter().any(|candidate| {
        candidate.kind == CandidateKind::ApiSource
            && candidate.available
            && (candidate.candidate_id == source_id
                || candidate
                    .candidate_id
                    .strip_prefix(source_id)
                    .is_some_and(|suffix| suffix.starts_with("::")))
    })
}

/// Validates a server-generated identifier formatted as a fixed prefix plus
/// the 32 hexadecimal characters emitted by `Uuid::simple()`.
pub fn valid_generated_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyMode {
    #[default]
    Direct,
    Common,
    Account,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountRoutingBlockReason {
    Disabled,
    NotInPool,
    Draining,
    SecretUnavailable,
    ProxyUnavailable,
    ReauthRequired,
    AuthError,
    Checkpoint,
    Captcha,
    SubscriptionForbidden,
    SubscriptionExpired,
    AccountUnhealthy,
    QuotaExhausted,
}

pub struct AccountOperationalInput<'a> {
    pub enabled: bool,
    pub in_pool: bool,
    pub draining: bool,
    pub secret_available: bool,
    pub proxy_available: bool,
    pub auth_state: AccountAuthState,
    pub health: AccountHealthState,
    pub subscription: &'a Subscription,
    pub quota: &'a QuotaSnapshot,
    pub last_error_code: Option<&'a str>,
    pub now_ms: u64,
    pub quota_stale_after_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AccountOperationalState {
    pub status: OperationalStatus,
    pub health: CandidateHealth,
    pub quota: CandidateQuota,
    pub routing_eligible: bool,
    pub routing_block_reason: Option<AccountRoutingBlockReason>,
}

/// Whether an account should remain instantiated as a runtime candidate.
///
/// An exhausted quota is a temporary scheduler condition rather than a broken
/// configuration. Keeping that candidate lets a quota refresh restore it in
/// place without rebuilding the gateway or dropping active work.
pub fn account_candidate_enabled(
    account_enabled: bool,
    routing_block_reason: Option<AccountRoutingBlockReason>,
) -> bool {
    account_enabled
        && matches!(
            routing_block_reason,
            None | Some(AccountRoutingBlockReason::QuotaExhausted)
        )
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaRefreshStatus {
    #[default]
    Pending,
    Refreshing,
    Updated,
    Failed,
    RequiresReauth,
}

pub fn quota_refresh_status(
    auth_state: AccountAuthState,
    quota: &QuotaSnapshot,
    refreshing: bool,
) -> QuotaRefreshStatus {
    if matches!(auth_state, AccountAuthState::RequiresReauth(_)) {
        QuotaRefreshStatus::RequiresReauth
    } else if refreshing {
        QuotaRefreshStatus::Refreshing
    } else if quota.error.is_some() {
        QuotaRefreshStatus::Failed
    } else if quota.updated_at_ms.is_some() {
        QuotaRefreshStatus::Updated
    } else {
        QuotaRefreshStatus::Pending
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum OperationalStatus {
    Rotation,
    QuotaWait,
    Unavailable,
    Disabled,
}

pub fn operational_status(
    enabled: bool,
    quota_wait: bool,
    configured_available: bool,
    runtime_available: Option<bool>,
) -> OperationalStatus {
    if !enabled {
        return OperationalStatus::Disabled;
    }
    if !configured_available {
        return OperationalStatus::Unavailable;
    }
    if quota_wait {
        return OperationalStatus::QuotaWait;
    }
    if runtime_available.unwrap_or(configured_available) {
        OperationalStatus::Rotation
    } else {
        OperationalStatus::Unavailable
    }
}

pub fn account_operational_state(input: AccountOperationalInput<'_>) -> AccountOperationalState {
    let health = account_candidate_health(
        input.auth_state,
        input.health,
        input.subscription.status,
        input.last_error_code,
    );
    let quota =
        CandidateQuota::from_snapshot(input.quota, input.now_ms, input.quota_stale_after_ms);
    let configured_available =
        !input.draining && input.secret_available && input.proxy_available && health.is_eligible();
    let status = operational_status(
        input.enabled,
        quota == CandidateQuota::Exhausted,
        configured_available,
        None,
    );
    let routing_block_reason = account_routing_block_reason(&input, health, quota);
    AccountOperationalState {
        status,
        health,
        quota,
        routing_eligible: routing_block_reason.is_none(),
        routing_block_reason,
    }
}

fn account_routing_block_reason(
    input: &AccountOperationalInput<'_>,
    health: CandidateHealth,
    quota: CandidateQuota,
) -> Option<AccountRoutingBlockReason> {
    if !input.enabled {
        return Some(AccountRoutingBlockReason::Disabled);
    }
    if !input.in_pool {
        return Some(AccountRoutingBlockReason::NotInPool);
    }
    if input.draining {
        return Some(AccountRoutingBlockReason::Draining);
    }
    if !input.secret_available {
        return Some(AccountRoutingBlockReason::SecretUnavailable);
    }
    if !input.proxy_available {
        return Some(AccountRoutingBlockReason::ProxyUnavailable);
    }
    match input.auth_state {
        AccountAuthState::RequiresReauth(_) => {
            return Some(AccountRoutingBlockReason::ReauthRequired)
        }
        AccountAuthState::Error => return Some(AccountRoutingBlockReason::AuthError),
        _ => {}
    }
    match input.last_error_code {
        Some("checkpoint" | "upstream_account_verification_required") => {
            return Some(AccountRoutingBlockReason::Checkpoint)
        }
        Some("captcha") => return Some(AccountRoutingBlockReason::Captcha),
        _ => {}
    }
    match input.subscription.status {
        SubscriptionStatus::Forbidden => {
            return Some(AccountRoutingBlockReason::SubscriptionForbidden)
        }
        SubscriptionStatus::Expired => return Some(AccountRoutingBlockReason::SubscriptionExpired),
        _ => {}
    }
    if !health.is_eligible() {
        return Some(AccountRoutingBlockReason::AccountUnhealthy);
    }
    (quota == CandidateQuota::Exhausted).then_some(AccountRoutingBlockReason::QuotaExhausted)
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
        if self.protocol_bindings.is_empty() {
            return if self.wire_api == wire_api {
                self.models.clone()
            } else {
                Vec::new()
            };
        }
        let expand_empty_models = self.protocol_bindings.len() == 1;
        let mut seen = std::collections::HashSet::new();
        let mut models = Vec::new();
        for binding in self
            .protocol_bindings
            .iter()
            .filter(|binding| binding.wire_api == wire_api)
        {
            let binding_models = if binding.model_ids.is_empty() && expand_empty_models {
                self.models.as_slice()
            } else {
                binding.model_ids.as_slice()
            };
            for model in binding_models {
                if seen.insert(model.to_ascii_lowercase()) {
                    models.push(model.clone());
                }
            }
        }
        models
    }

    pub fn supports_wire_api(&self, wire_api: WireApi) -> bool {
        !self.models_for_wire_api(wire_api).is_empty()
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
    #[serde(default)]
    pub economics: crate::quota::QuotaEconomicsSummary,
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

pub const PROFILE_KEY_ROTATION_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientWireApi {
    Responses,
    ChatCompletions,
    Messages,
    Images,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileKeyRotation {
    pub schema_version: u16,
    pub rotation_id: String,
    pub key_id: String,
    pub base_url: String,
    pub secret: String,
}

impl fmt::Debug for ProfileKeyRotation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProfileKeyRotation")
            .field("schema_version", &self.schema_version)
            .field("rotation_id", &self.rotation_id)
            .field("key_id", &self.key_id)
            .field("base_url", &self.base_url)
            .field("secret", &"[redacted]")
            .finish()
    }
}

pub const CONFIGURATION_PRESET_FORMAT: &str = "zenith-relay-configuration";
pub const CONFIGURATION_PRESET_SCHEMA_VERSION: u16 = 3;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPreset {
    pub format: String,
    pub schema_version: u16,
    pub settings: ConfigurationPresetSettings,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigurationPresetSettings {
    pub sources: Vec<SourcePresetRule>,
    pub accounts: Vec<AccountPresetRule>,
    pub routing: PresetRoutingPolicy,
    pub quota: PresetQuotaPolicy,
    pub hidden_models: Vec<String>,
    pub model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
    pub model_reasoning_allowed_levels: BTreeMap<String, Vec<String>>,
    /// Whether the preset explicitly supplied `modelReasoningAllowedLevels`.
    ///
    /// Resolved configuration settings always set this to `true`; it is false
    /// only while importing a backward-compatible sparse preset.
    pub model_reasoning_allowed_levels_present: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ConfigurationPresetSettingsWire {
    sources: Vec<SourcePresetRule>,
    accounts: Vec<AccountPresetRule>,
    routing: PresetRoutingPolicy,
    quota: PresetQuotaPolicy,
    hidden_models: Vec<String>,
    model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
    #[serde(
        default,
        alias = "modelReasoningOverrides",
        deserialize_with = "deserialize_optional_model_reasoning_allowed_levels"
    )]
    model_reasoning_allowed_levels: Option<BTreeMap<String, Vec<String>>>,
}

fn deserialize_optional_model_reasoning_allowed_levels<'de, D>(
    deserializer: D,
) -> Result<Option<BTreeMap<String, Vec<String>>>, D::Error>
where
    D: Deserializer<'de>,
{
    crate::deserialize_model_reasoning_allowed_levels(deserializer).map(Some)
}

impl<'de> Deserialize<'de> for ConfigurationPresetSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ConfigurationPresetSettingsWire::deserialize(deserializer)?;
        let model_reasoning_allowed_levels_present = wire.model_reasoning_allowed_levels.is_some();
        Ok(Self {
            sources: wire.sources,
            accounts: wire.accounts,
            routing: wire.routing,
            quota: wire.quota,
            hidden_models: wire.hidden_models,
            model_price_overrides: wire.model_price_overrides,
            model_reasoning_allowed_levels: wire.model_reasoning_allowed_levels.unwrap_or_default(),
            model_reasoning_allowed_levels_present,
        })
    }
}

impl Serialize for ConfigurationPresetSettings {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct(
            "ConfigurationPresetSettings",
            6 + usize::from(self.model_reasoning_allowed_levels_present),
        )?;
        state.serialize_field("sources", &self.sources)?;
        state.serialize_field("accounts", &self.accounts)?;
        state.serialize_field("routing", &self.routing)?;
        state.serialize_field("quota", &self.quota)?;
        state.serialize_field("hiddenModels", &self.hidden_models)?;
        state.serialize_field("modelPriceOverrides", &self.model_price_overrides)?;
        if self.model_reasoning_allowed_levels_present {
            state.serialize_field(
                "modelReasoningAllowedLevels",
                &self.model_reasoning_allowed_levels,
            )?;
        }
        state.end()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SourcePresetRule {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub wire_api: WireApi,
    #[serde(default)]
    pub protocol_bindings: Vec<SourceProtocolBinding>,
    pub enabled: bool,
    pub in_pool: bool,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    #[serde(default)]
    pub recovery_delay_seconds: u64,
    #[serde(default)]
    pub model_price_overrides: BTreeMap<String, ApiModelPriceOverride>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountPresetRule {
    pub id: String,
    pub identity_hint: String,
    pub enabled: bool,
    pub in_pool: bool,
    pub allowed_models: Vec<String>,
    pub excluded_models: Vec<String>,
    pub priority: i32,
    pub weight: u32,
    pub proxy_id: Option<String>,
    #[serde(default)]
    pub bypass_common_proxy: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresetRoutingPolicy {
    pub max_retry_candidates: u8,
    #[serde(default = "default_cooldown_after_failures")]
    pub cooldown_after_failures: u8,
    #[serde(default = "default_keep_last_candidate_available")]
    pub keep_last_candidate_available: bool,
    pub routing_strategy: RoutingStrategy,
    pub subscription_plan_order: Vec<String>,
    pub default_service_tier: DefaultServiceTier,
    pub image_base_model: Option<String>,
}

fn default_cooldown_after_failures() -> u8 {
    crate::DEFAULT_COOLDOWN_AFTER_FAILURES
}

fn default_keep_last_candidate_available() -> bool {
    crate::DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresetQuotaPolicy {
    pub request_timeout_seconds: u64,
    pub account_proxy_required: bool,
    pub common_proxy_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPresetDocument {
    pub revision: String,
    pub preset: ConfigurationPreset,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPresetPreviewInput {
    pub preset: ConfigurationPreset,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPresetApplyInput {
    pub base_revision: String,
    pub preset: ConfigurationPreset,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPresetChange {
    pub path: String,
    pub before: Value,
    pub after: Value,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPresetPreview {
    pub base_revision: String,
    pub preset: ConfigurationPreset,
    pub changes: Vec<ConfigurationPresetChange>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPresetApplyResult {
    pub previous_revision: String,
    pub revision: String,
    pub changes: Vec<ConfigurationPresetChange>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStateSnapshot {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration_revision: Option<String>,
    pub runtime_target: RuntimeTargetSummary,
    pub gateway: GatewaySummary,
    pub platform: String,
    pub capabilities: Capabilities,
    pub sources: Vec<SourceSummary>,
    pub accounts: Vec<AccountSummary>,
    pub automations: Vec<WakeTask>,
    pub wake_history: Vec<WakeHistory>,
    pub warnings: Vec<String>,
}

pub fn pool_model_summaries(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    hidden_models: &[String],
) -> Vec<ModelSummary> {
    let mut models = BTreeMap::<String, PoolModel>::new();
    let mut upstream_order = 0usize;
    for source in sources.iter().filter(|source| {
        source.enabled && source.in_pool && !source.draining && source.secret_available
    }) {
        let response_models = source.models_for_wire_api(WireApi::Responses);
        add_member_models(
            &mut models,
            &format!("source:{}", source.id),
            &response_models,
            &source.allowed_models,
            &source.excluded_models,
            &mut upstream_order,
        );
    }
    for account in accounts.iter().filter(|account| {
        account.enabled
            && account.in_pool
            && !account.draining
            && account.secret_available
            && account.proxy_available
    }) {
        add_member_models(
            &mut models,
            &format!("account:{}", account.id),
            &account.models,
            &account.allowed_models,
            &account.excluded_models,
            &mut upstream_order,
        );
    }

    let mut summaries = models
        .into_values()
        .map(|model| {
            let id = model.id.clone();
            let price = api_model_price(&id);
            let enabled = !hidden_models
                .iter()
                .any(|hidden| hidden.eq_ignore_ascii_case(&id));
            (
                model.upstream_order,
                ModelSummary {
                    enabled,
                    codex_visible: enabled && codex_model_is_picker_eligible(&id),
                    codex_display_name: codex_model_display_name(&id),
                    id,
                    member_count: model.members.len(),
                    catalog_rank: price.map(|price| price.catalog_rank),
                    input_micro_usd_per_million: price
                        .map(|price| price.input_micro_usd_per_million),
                    cached_input_micro_usd_per_million: price
                        .map(|price| price.cached_input_micro_usd_per_million),
                    cache_write_5m_micro_usd_per_million: price
                        .and_then(|price| price.cache_write_5m_micro_usd_per_million),
                    cache_write_1h_micro_usd_per_million: price
                        .and_then(|price| price.cache_write_1h_micro_usd_per_million),
                    output_micro_usd_per_million: price
                        .map(|price| price.output_micro_usd_per_million),
                    custom_price: false,
                    reasoning_levels: Vec::new(),
                    reasoning_allowed_levels: Vec::new(),
                    reasoning_configurable: false,
                },
            )
        })
        .collect::<Vec<_>>();
    summaries.sort_by_key(|(upstream_order, _)| *upstream_order);
    summaries.into_iter().map(|(_, summary)| summary).collect()
}

struct PoolModel {
    id: String,
    members: BTreeSet<String>,
    upstream_order: usize,
}

fn add_member_models(
    models: &mut BTreeMap<String, PoolModel>,
    member_id: &str,
    member_models: &[String],
    allowed_models: &[String],
    excluded_models: &[String],
    upstream_order: &mut usize,
) {
    let rules = ModelRules {
        allowed: allowed_models.iter().cloned().collect(),
        excluded: excluded_models.iter().cloned().collect(),
    };
    for model in member_models {
        let model_order = *upstream_order;
        *upstream_order = upstream_order.saturating_add(1);
        if !rules.allows(model) {
            continue;
        }
        let key = model.to_ascii_lowercase();
        let entry = models.entry(key).or_insert_with(|| PoolModel {
            id: model.clone(),
            members: BTreeSet::new(),
            upstream_order: model_order,
        });
        entry.members.insert(member_id.to_string());
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSummary {
    pub id: i64,
    pub request_id: String,
    pub candidate_kind: String,
    pub candidate_hint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub routing: Option<RoutingDiagnostics>,
    pub requested_model: Option<String>,
    pub resolved_model: Option<String>,
    pub wire_api: WireApi,
    #[serde(default)]
    pub service_tier: crate::DefaultServiceTier,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub applied_service_tier: Option<crate::DefaultServiceTier>,
    pub success: bool,
    pub http_status: u16,
    pub error_category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_origin: Option<crate::ErrorOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use: Option<crate::ToolUseDiagnostics>,
    pub latency_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttft_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_ms: Option<u64>,
    pub input_tokens: Option<u64>,
    pub cached_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write_input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub api_equivalent: ApiEquivalentSummary,
    pub created_at_ms: u64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    pub requests: u64,
    pub successful_requests: u64,
    pub latency_ms: u64,
    pub ttft_ms: u64,
    pub ttft_samples: u64,
    pub generation_ms: u64,
    pub generation_samples: u64,
    pub generation_output_tokens: u64,
    pub input_tokens: u64,
    pub cached_input_tokens: u64,
    pub cached_input_samples: u64,
    #[serde(default)]
    pub cache_write_input_tokens: u64,
    #[serde(default)]
    pub cache_write_input_samples: u64,
    pub reasoning_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub speed_output_tokens: u64,
    pub speed_duration_ms: u64,
    pub api_equivalent: ApiEquivalentSummary,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageGroup {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub totals: UsageTotals,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageBucket {
    pub start_ms: u64,
    pub totals: UsageTotals,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsagePage {
    pub events: Vec<UsageSummary>,
    pub total: u64,
    pub page: u32,
    pub page_size: u32,
    pub total_pages: u32,
    #[serde(default)]
    pub totals: UsageTotals,
    #[serde(default)]
    pub models: Vec<UsageGroup>,
    #[serde(default)]
    pub pool_members: Vec<UsageGroup>,
    #[serde(default)]
    pub buckets: Vec<UsageBucket>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsageRange {
    Daily,
    Weekly,
    Monthly,
    Custom,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub page_size: u32,
    pub range: Option<UsageRange>,
    pub from_ms: Option<u64>,
    pub to_ms: Option<u64>,
    pub bucket_ms: Option<u64>,
    pub model_query: Option<String>,
    pub source_or_account_query: Option<String>,
    pub wire_api: Option<WireApi>,
    pub success: Option<bool>,
    pub error_category: Option<String>,
    pub request_id_query: Option<String>,
}

impl UsageQuery {
    pub fn normalize_pagination(&mut self) {
        self.page = self.page.max(1);
        self.page_size = if self.page_size == 0 {
            50
        } else {
            self.page_size.clamp(1, 200)
        };
        self.bucket_ms = self.bucket_ms.filter(|value| *value >= 60_000);
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GatewayDiagnostic {
    pub stream: bool,
    pub model: String,
    pub latency_ms: u64,
    pub bytes_received: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub stage: String,
    pub retryable: bool,
    pub request_id: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorEnvelope {
    pub error: ApiError,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quota::{QuotaEconomicsSummary, QuotaSnapshot, QuotaWindow, QuotaWindowKind};
    use crate::{
        ActiveModelRuntime, ApiEquivalentSummary, CandidateKind, CandidateRuntimeSnapshot,
        MessagesReasoningMode, SourceAdapter,
    };

    fn runtime_candidate(
        candidate_id: &str,
        kind: CandidateKind,
        available: bool,
    ) -> CandidateRuntimeSnapshot {
        CandidateRuntimeSnapshot {
            candidate_id: candidate_id.into(),
            kind,
            available,
            in_flight: 0,
            active_request_count: 0,
            active_models: Vec::<ActiveModelRuntime>::new(),
            last_used_at_ms: None,
            next_retry_at_ms: None,
            half_open: false,
            dispatches: 0,
        }
    }

    fn account_summary(in_pool: bool, models: &[&str]) -> AccountSummary {
        AccountSummary {
            id: "account".into(),
            label: "Account".into(),
            identity_hint: "account".into(),
            enabled: true,
            in_pool,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            auth_state: AccountAuthState::Active,
            health: "healthy".into(),
            models: models.iter().map(ToString::to_string).collect(),
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            api_equivalent: ApiEquivalentSummary::default(),
            economics: QuotaEconomicsSummary::default(),
            subscription: Subscription::default(),
            quota: QuotaSnapshot::default(),
            quota_refresh_status: QuotaRefreshStatus::default(),
            secret_available: true,
            remote_location: None,
            proxy_mode: ProxyMode::Direct,
            proxy_available: true,
            proxy_id: None,
            routing_block_reason: None,
            last_error_code: None,
        }
    }

    #[test]
    fn usage_query_pagination_uses_bounded_defaults() {
        let mut query = UsageQuery {
            page: 0,
            page_size: 0,
            bucket_ms: Some(59_999),
            ..Default::default()
        };
        query.normalize_pagination();
        assert_eq!(query.page, 1);
        assert_eq!(query.page_size, 50);
        assert_eq!(query.bucket_ms, None);

        query.page = 9;
        query.page_size = 999;
        query.bucket_ms = Some(60_000);
        query.normalize_pagination();
        assert_eq!(query.page, 9);
        assert_eq!(query.page_size, 200);
        assert_eq!(query.bucket_ms, Some(60_000));
    }

    #[test]
    fn runtime_and_native_account_helpers_keep_summary_rules_shared() {
        let runtime = [
            runtime_candidate("source::messages", CandidateKind::ApiSource, true),
            runtime_candidate("source::responses", CandidateKind::ApiSource, false),
            runtime_candidate("source", CandidateKind::OAuthAccount, true),
        ];
        assert!(source_runtime_available(&runtime, "source"));
        assert!(!source_runtime_available(&runtime, "missing"));
        assert!(!source_runtime_available(&runtime, "sour"));

        let accounts = [
            account_summary(true, &["GPT-5"]),
            account_summary(false, &["other"]),
        ];
        assert!(model_has_native_account_route(&accounts, "gpt-5"));
        assert!(!model_has_native_account_route(&accounts, "other"));
    }

    #[test]
    fn generated_ids_require_the_expected_prefix_and_hex_suffix() {
        assert!(valid_generated_id(
            "batch_0123456789abcdef0123456789ABCDEF",
            "batch_"
        ));
        assert!(!valid_generated_id("batch_0123456789abcdef", "batch_"));
        assert!(!valid_generated_id(
            "batch_0123456789abcdef0123456789abcdeg",
            "batch_"
        ));
        assert!(!valid_generated_id(
            "import_0123456789abcdef0123456789abcdef",
            "batch_"
        ));
    }

    #[test]
    fn model_summaries_apply_member_rules_hidden_state_and_catalog_order() {
        let source = SourceSummary {
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec![
                "gpt-old".into(),
                "gpt-5.4-mini".into(),
                "gpt-5.4".into(),
                "gpt-future-codex".into(),
            ],
            allowed_models: Vec::new(),
            excluded_models: vec!["gpt-old".into()],
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            api_equivalent: ApiEquivalentSummary::default(),
            secret_available: true,
            last_error_code: None,
        };

        let models = pool_model_summaries(&[source], &[], &["GPT-5.4-MINI".into()]);

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-5.4-mini", "gpt-5.4", "gpt-future-codex"]
        );
        assert!(!models[0].enabled);
        assert!(models[1].enabled);
        assert!(models[2].enabled);
        assert_eq!(models[1].member_count, 1);
        assert!(models[1].output_micro_usd_per_million.is_some());
        assert!(models[2].catalog_rank.is_none());
        assert!(models[2].output_micro_usd_per_million.is_none());
    }

    #[test]
    fn pool_model_summaries_exclude_messages_only_source_models() {
        let source = SourceSummary {
            id: "source_1".into(),
            name: "Mixed source".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            wire_api: WireApi::Responses,
            protocol_bindings: vec![
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    model_ids: vec!["gpt-routed".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    model_ids: vec!["claude-native".into()],
                },
            ],
            models: vec!["gpt-routed".into(), "claude-native".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            api_equivalent: ApiEquivalentSummary::default(),
            secret_available: true,
            last_error_code: None,
        };

        let models = pool_model_summaries(&[source], &[], &[]);

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-routed"]
        );
    }

    #[test]
    fn source_summary_preserves_legacy_and_native_protocol_model_boundaries() {
        let legacy = SourceSummary {
            id: "legacy".into(),
            name: "Legacy".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            models: vec!["gpt-legacy".into()],
            allowed_models: Vec::new(),
            excluded_models: Vec::new(),
            priority: 0,
            weight: 1,
            recovery_delay_seconds: 0,
            model_price_overrides: BTreeMap::new(),
            detected_model_prices: BTreeMap::new(),
            api_equivalent: ApiEquivalentSummary::default(),
            secret_available: true,
            last_error_code: None,
        };
        assert_eq!(
            legacy.models_for_wire_api(WireApi::Responses),
            ["gpt-legacy"]
        );
        assert!(!legacy.supports_wire_api(WireApi::Messages));

        let mixed = SourceSummary {
            protocol_bindings: vec![
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    model_ids: vec!["gpt-native".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::ResponsesToMessages,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    model_ids: vec!["claude-bridged".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    model_ids: vec!["claude-native".into()],
                },
            ],
            models: vec![
                "gpt-native".into(),
                "claude-bridged".into(),
                "claude-native".into(),
            ],
            ..legacy
        };
        assert_eq!(
            mixed.models_for_wire_api(WireApi::Responses),
            ["gpt-native", "claude-bridged"]
        );
        assert_eq!(
            mixed.models_for_wire_api(WireApi::Messages),
            ["claude-native"]
        );
        assert!(mixed.supports_wire_api(WireApi::Messages));
        assert!(!mixed.supports_wire_api(WireApi::ChatCompletions));

        let unconfirmed = SourceSummary {
            protocol_bindings: vec![
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    model_ids: vec!["gpt-native".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    model_ids: Vec::new(),
                },
            ],
            ..mixed
        };
        assert!(unconfirmed
            .models_for_wire_api(WireApi::Messages)
            .is_empty());
        assert!(!unconfirmed.supports_wire_api(WireApi::Messages));
    }

    #[test]
    fn legacy_preset_routing_defaults_new_cooldown_policy() {
        let policy: PresetRoutingPolicy = serde_json::from_str(
            r#"{"maxRetryCandidates":3,"routingStrategy":"adaptive","subscriptionPlanOrder":[],"defaultServiceTier":"standard","imageBaseModel":null}"#,
        )
        .unwrap();

        assert_eq!(
            policy.cooldown_after_failures,
            crate::DEFAULT_COOLDOWN_AFTER_FAILURES
        );
        assert_eq!(
            policy.keep_last_candidate_available,
            crate::DEFAULT_KEEP_LAST_CANDIDATE_AVAILABLE
        );
    }

    #[test]
    fn usage_summary_accepts_servers_without_reasoning_telemetry() {
        let summary: UsageSummary = serde_json::from_str(
            r#"{"id":1,"requestId":"req","localKeyId":"key","candidateKind":"source","candidateHint":"abc","requestedModel":null,"resolvedModel":null,"wireApi":"responses","success":true,"httpStatus":200,"errorCategory":null,"latencyMs":1,"inputTokens":2,"cachedInputTokens":null,"outputTokens":3,"totalTokens":5,"createdAtMs":1}"#,
        )
        .unwrap();

        assert_eq!(summary.reasoning_tokens, None);
        assert_eq!(summary.ttft_ms, None);
        assert!(!serde_json::to_value(&summary)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("localKeyId"));

        let mut legacy_totals = serde_json::to_value(UsageTotals::default()).unwrap();
        let fields = legacy_totals.as_object_mut().unwrap();
        fields.remove("cacheWriteInputTokens");
        fields.remove("cacheWriteInputSamples");
        let totals: UsageTotals = serde_json::from_value(legacy_totals).unwrap();
        assert_eq!(totals.cache_write_input_tokens, 0);
        assert_eq!(totals.cache_write_input_samples, 0);
    }

    #[test]
    fn operational_status_has_one_backend_precedence() {
        assert_eq!(
            operational_status(false, false, true, Some(true)),
            OperationalStatus::Disabled
        );
        assert_eq!(
            operational_status(true, true, false, Some(true)),
            OperationalStatus::Unavailable
        );
        assert_eq!(
            operational_status(true, true, true, Some(false)),
            OperationalStatus::QuotaWait
        );
        assert_eq!(
            operational_status(true, false, true, Some(false)),
            OperationalStatus::Unavailable
        );
        assert_eq!(
            operational_status(true, false, true, None),
            OperationalStatus::Rotation
        );
    }

    #[test]
    fn account_operational_state_is_shared_and_does_not_invent_fresh_exhaustion() {
        let subscription = Subscription {
            plan_type: Some("plus".into()),
            active_until_ms: None,
            status: SubscriptionStatus::Active,
            updated_at_ms: Some(1),
        };
        let mut quota = QuotaSnapshot {
            primary: Some(QuotaWindow {
                kind: QuotaWindowKind::Primary,
                available_basis_points: Some(0),
                explicitly_full: None,
                reset_at_ms: None,
                window_minutes: None,
                observed_at_ms: 1,
                full_transition_fingerprint: None,
            }),
            updated_at_ms: Some(1),
            ..Default::default()
        };
        let state = account_operational_state(AccountOperationalInput {
            enabled: true,
            in_pool: true,
            draining: false,
            secret_available: true,
            proxy_available: true,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            subscription: &subscription,
            quota: &quota,
            last_error_code: None,
            now_ms: 1_000,
            quota_stale_after_ms: 10,
        });
        assert_eq!(state.quota, CandidateQuota::Stale);
        assert_eq!(state.status, OperationalStatus::Rotation);
        assert!(state.routing_eligible);
        assert_eq!(state.routing_block_reason, None);

        quota.updated_at_ms = Some(1_000);
        quota.primary.as_mut().unwrap().available_basis_points = Some(5_000);
        let state = account_operational_state(AccountOperationalInput {
            enabled: true,
            in_pool: false,
            draining: false,
            secret_available: true,
            proxy_available: true,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            subscription: &subscription,
            quota: &quota,
            last_error_code: None,
            now_ms: 1_000,
            quota_stale_after_ms: 10,
        });
        assert_eq!(state.status, OperationalStatus::Rotation);
        assert!(!state.routing_eligible);
        assert_eq!(
            state.routing_block_reason,
            Some(AccountRoutingBlockReason::NotInPool)
        );

        quota.primary.as_mut().unwrap().available_basis_points = Some(0);
        let state = account_operational_state(AccountOperationalInput {
            enabled: true,
            in_pool: true,
            draining: false,
            secret_available: true,
            proxy_available: true,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            subscription: &subscription,
            quota: &quota,
            last_error_code: None,
            now_ms: 1_000,
            quota_stale_after_ms: 10,
        });
        assert_eq!(state.quota, CandidateQuota::Exhausted);
        assert_eq!(state.status, OperationalStatus::QuotaWait);
        assert_eq!(
            state.routing_block_reason,
            Some(AccountRoutingBlockReason::QuotaExhausted)
        );
        assert!(account_candidate_enabled(true, state.routing_block_reason));
    }

    #[test]
    fn unavailable_credentials_always_win_over_pending_quota() {
        let subscription = Subscription {
            plan_type: Some("plus".into()),
            active_until_ms: None,
            status: SubscriptionStatus::Active,
            updated_at_ms: Some(1),
        };
        let state = account_operational_state(AccountOperationalInput {
            enabled: true,
            in_pool: true,
            draining: false,
            secret_available: false,
            proxy_available: true,
            auth_state: AccountAuthState::Active,
            health: AccountHealthState::Healthy,
            subscription: &subscription,
            quota: &QuotaSnapshot::default(),
            last_error_code: None,
            now_ms: 1_000,
            quota_stale_after_ms: 10,
        });
        assert_eq!(state.status, OperationalStatus::Unavailable);
        assert_eq!(
            state.routing_block_reason,
            Some(AccountRoutingBlockReason::SecretUnavailable)
        );
    }

    #[test]
    fn quota_refresh_status_has_one_visible_precedence() {
        let mut quota = QuotaSnapshot::default();
        assert_eq!(
            quota_refresh_status(AccountAuthState::Active, &quota, false),
            QuotaRefreshStatus::Pending
        );
        assert_eq!(
            quota_refresh_status(AccountAuthState::Active, &quota, true),
            QuotaRefreshStatus::Refreshing
        );
        quota.updated_at_ms = Some(1);
        assert_eq!(
            quota_refresh_status(AccountAuthState::Active, &quota, false),
            QuotaRefreshStatus::Updated
        );
        assert_eq!(
            quota_refresh_status(
                AccountAuthState::RequiresReauth(crate::accounts::ReauthReason::InvalidGrant),
                &quota,
                true,
            ),
            QuotaRefreshStatus::RequiresReauth
        );
    }
}
