use super::Capabilities;
use crate::{
    api_model_price,
    automations::{WakeHistory, WakeTask},
    codex_model_display_name, codex_model_is_picker_eligible, official_image_request_prices,
    ApiModelPriceOverride, DefaultServiceTier, ModelRules, RoutingStrategy, SourceProtocolBinding,
    WireApi,
};
mod account;
mod model;
mod routing;
mod usage;

pub use account::{
    api_equivalent_projection_window, model_has_native_account_route, AccountSummary,
    QuotaWindowUsage, RemoteAccountLocation, RevealedAccountIdentity, SourceSummary,
};
pub use model::{
    apply_model_display_order, apply_model_reasoning_summary, apply_model_speed_summary,
    model_has_api_source_route, pooled_source_runtime_available, source_runtime_available,
    GatewaySummary, ModelSummary,
};
pub use routing::{
    account_candidate_enabled, account_operational_state, operational_status, quota_refresh_status,
    AccountOperationalInput, AccountOperationalState, AccountRoutingBlockReason, OperationalStatus,
    ProxyMode, QuotaRefreshStatus,
};
pub use usage::{
    UsageBucket, UsageGroup, UsagePage, UsageQuery, UsageRange, UsageSummary, UsageTotals,
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

/// Validates a server-generated identifier formatted as a fixed prefix plus
/// the 32 hexadecimal characters emitted by `Uuid::simple()`.
pub fn valid_generated_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == 32 && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

pub const PROFILE_KEY_ROTATION_SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientWireApi {
    Responses,
    ChatCompletions,
    Messages,
    Gemini,
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
    pub model_service_tier_overrides: BTreeMap<String, DefaultServiceTier>,
    pub model_display_order: Vec<String>,
    /// Whether the preset explicitly supplied `modelReasoningAllowedLevels`.
    ///
    /// Resolved configuration settings always set this to `true`; it is false
    /// only while importing a backward-compatible sparse preset.
    pub model_reasoning_allowed_levels_present: bool,
    pub model_service_tier_overrides_present: bool,
    pub model_display_order_present: bool,
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
    #[serde(default)]
    model_service_tier_overrides: Option<BTreeMap<String, DefaultServiceTier>>,
    #[serde(default)]
    model_display_order: Option<Vec<String>>,
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
        let model_service_tier_overrides_present = wire.model_service_tier_overrides.is_some();
        let model_display_order_present = wire.model_display_order.is_some();
        Ok(Self {
            sources: wire.sources,
            accounts: wire.accounts,
            routing: wire.routing,
            quota: wire.quota,
            hidden_models: wire.hidden_models,
            model_price_overrides: wire.model_price_overrides,
            model_service_tier_overrides: wire.model_service_tier_overrides.unwrap_or_default(),
            model_display_order: wire.model_display_order.unwrap_or_default(),
            model_reasoning_allowed_levels: wire.model_reasoning_allowed_levels.unwrap_or_default(),
            model_reasoning_allowed_levels_present,
            model_service_tier_overrides_present,
            model_display_order_present,
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
            6 + usize::from(self.model_reasoning_allowed_levels_present)
                + usize::from(self.model_service_tier_overrides_present)
                + usize::from(self.model_display_order_present),
        )?;
        state.serialize_field("sources", &self.sources)?;
        state.serialize_field("accounts", &self.accounts)?;
        state.serialize_field("routing", &self.routing)?;
        state.serialize_field("quota", &self.quota)?;
        state.serialize_field("hiddenModels", &self.hidden_models)?;
        state.serialize_field("modelPriceOverrides", &self.model_price_overrides)?;
        if self.model_service_tier_overrides_present {
            state.serialize_field(
                "modelServiceTierOverrides",
                &self.model_service_tier_overrides,
            )?;
        }
        if self.model_display_order_present {
            state.serialize_field("modelDisplayOrder", &self.model_display_order)?;
        }
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
        let pool_models = source.models_for_any_wire_api();
        add_member_models(
            &mut models,
            &format!("source:{}", source.id),
            &pool_models,
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
            let image_request_prices = official_image_request_prices(&id);
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
                    image_request_prices,
                    custom_price: false,
                    reasoning_levels: Vec::new(),
                    reasoning_supported_levels: Vec::new(),
                    reasoning_allowed_levels: Vec::new(),
                    reasoning_configurable: false,
                    speed_supported: false,
                    speed_tier: DefaultServiceTier::Standard,
                    speed_configurable: false,
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
    use crate::{
        accounts::{AccountAuthState, AccountHealthState},
        quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription, SubscriptionStatus},
        CandidateHealth, CandidateQuota,
    };
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
            model_retries: Vec::new(),
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
            quota_window_usage: None,
            purchase_cost_micro_usd: None,
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
        assert!(pooled_source_runtime_available(&runtime, "source"));
        let responses = [runtime_candidate(
            "source::responses_to_messages",
            CandidateKind::ApiSource,
            true,
        )];
        assert!(pooled_source_runtime_available(&responses, "source"));
        let legacy = [runtime_candidate("source", CandidateKind::ApiSource, true)];
        assert!(pooled_source_runtime_available(&legacy, "source"));
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
    fn model_reasoning_summary_keeps_provider_hints_manual() {
        let mut model = ModelSummary {
            enabled: true,
            codex_visible: true,
            codex_display_name: String::new(),
            id: "gpt-test".into(),
            member_count: 1,
            catalog_rank: None,
            input_micro_usd_per_million: None,
            cached_input_micro_usd_per_million: None,
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: None,
            image_request_prices: Vec::new(),
            custom_price: false,
            reasoning_levels: Vec::new(),
            reasoning_supported_levels: Vec::new(),
            reasoning_allowed_levels: Vec::new(),
            reasoning_configurable: false,
            speed_supported: false,
            speed_tier: DefaultServiceTier::Standard,
            speed_configurable: false,
        };

        apply_model_reasoning_summary(
            &mut model,
            Some(vec!["high".into()]),
            Some(&["ultra".into()]),
            false,
        );
        assert!(model.reasoning_levels.is_empty());
        assert_eq!(model.reasoning_supported_levels, ["high"]);
        assert!(model.reasoning_allowed_levels.is_empty());
        assert!(!model.reasoning_configurable);

        apply_model_reasoning_summary(&mut model, None, None, false);
        assert!(model.reasoning_levels.is_empty());
        assert!(model.reasoning_supported_levels.is_empty());
        assert!(model.reasoning_allowed_levels.is_empty());
        assert!(!model.reasoning_configurable);

        apply_model_reasoning_summary(&mut model, Some(vec!["high".into()]), None, true);
        assert_eq!(model.reasoning_levels, ["high"]);
        assert_eq!(model.reasoning_supported_levels, ["high"]);
        assert_eq!(model.reasoning_allowed_levels, ["high"]);
        assert!(model.reasoning_configurable);

        apply_model_reasoning_summary(&mut model, Some(Vec::new()), Some(&["max".into()]), true);
        assert_eq!(model.reasoning_levels, ["max"]);
        assert!(model.reasoning_supported_levels.is_empty());
        assert_eq!(model.reasoning_allowed_levels, ["max"]);
        assert!(model.reasoning_configurable);

        model.id = "gpt-5.6-terra".into();
        apply_model_reasoning_summary(&mut model, Some(Vec::new()), None, true);
        assert!(model.reasoning_supported_levels.is_empty());
        assert!(model.reasoning_allowed_levels.is_empty());

        apply_model_reasoning_summary(&mut model, Some(vec!["ultra".into()]), None, true);
        assert_eq!(model.reasoning_supported_levels, ["ultra"]);
        assert_eq!(model.reasoning_allowed_levels, ["ultra"]);
    }

    #[test]
    fn anthropic_max_is_advertised_as_ultra_for_api_sources() {
        let mut model = ModelSummary {
            enabled: true,
            codex_visible: true,
            codex_display_name: String::new(),
            id: "claude-opus-4-8".into(),
            member_count: 1,
            catalog_rank: None,
            input_micro_usd_per_million: None,
            cached_input_micro_usd_per_million: None,
            cache_write_5m_micro_usd_per_million: None,
            cache_write_1h_micro_usd_per_million: None,
            output_micro_usd_per_million: None,
            image_request_prices: Vec::new(),
            custom_price: false,
            reasoning_levels: Vec::new(),
            reasoning_supported_levels: Vec::new(),
            reasoning_allowed_levels: Vec::new(),
            reasoning_configurable: false,
            speed_supported: false,
            speed_tier: DefaultServiceTier::Standard,
            speed_configurable: false,
        };
        apply_model_reasoning_summary(
            &mut model,
            Some(vec!["low".into(), "max".into()]),
            None,
            true,
        );
        assert_eq!(model.reasoning_supported_levels, ["low", "max", "ultra"]);
        assert_eq!(model.reasoning_levels, ["low", "max", "ultra"]);
    }

    #[test]
    fn api_source_reasoning_route_requires_an_active_responses_source() {
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
            models: vec!["gpt-test".into()],
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
        assert!(model_has_api_source_route(
            std::slice::from_ref(&source),
            "GPT-TEST"
        ));

        let mut unavailable = source.clone();
        unavailable.secret_available = false;
        assert!(!model_has_api_source_route(&[unavailable], "gpt-test"));

        let mut outside_pool = source;
        outside_pool.in_pool = false;
        assert!(!model_has_api_source_route(&[outside_pool], "gpt-test"));
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
    fn pool_model_summaries_include_the_runtime_messages_bridge() {
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
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["gpt-routed".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
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
            ["gpt-routed", "claude-native"]
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
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["gpt-native".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Responses,
                    adapter: SourceAdapter::ResponsesToMessages,
                    reasoning_mode: MessagesReasoningMode::Adaptive,
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["claude-bridged".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
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
                    cache_write_ttl: Default::default(),
                    model_ids: vec!["gpt-native".into()],
                },
                SourceProtocolBinding {
                    wire_api: WireApi::Messages,
                    adapter: SourceAdapter::Native,
                    reasoning_mode: MessagesReasoningMode::Disabled,
                    cache_write_ttl: Default::default(),
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
                provider_cycle_id: None,
                window_start_ms: None,
                available_basis_points: Some(0),
                explicitly_full: None,
                reset_at_ms: None,
                window_minutes: None,
                observed_at_ms: 1,
                full_transition_fingerprint: None,
                exhaustion_transition_fingerprint: None,
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
    fn expired_chatgpt_entitlement_does_not_block_working_codex_account() {
        let subscription = Subscription {
            plan_type: Some("business".into()),
            active_until_ms: Some(900),
            status: SubscriptionStatus::Expired,
            updated_at_ms: Some(900),
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
            quota: &QuotaSnapshot {
                primary: Some(QuotaWindow {
                    kind: QuotaWindowKind::Primary,
                    provider_cycle_id: None,
                    window_start_ms: None,
                    available_basis_points: Some(8_000),
                    explicitly_full: None,
                    reset_at_ms: None,
                    window_minutes: None,
                    observed_at_ms: 1_000,
                    full_transition_fingerprint: None,
                    exhaustion_transition_fingerprint: None,
                }),
                updated_at_ms: Some(1_000),
                ..Default::default()
            },
            last_error_code: None,
            now_ms: 1_000,
            quota_stale_after_ms: 10_000,
        });

        assert_eq!(state.health, CandidateHealth::Healthy);
        assert!(state.routing_eligible);
        assert_eq!(state.routing_block_reason, None);
    }

    #[test]
    fn forbidden_chatgpt_subscription_still_blocks_routing() {
        let subscription = Subscription {
            plan_type: Some("business".into()),
            status: SubscriptionStatus::Forbidden,
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
            quota: &QuotaSnapshot::default(),
            last_error_code: None,
            now_ms: 1_000,
            quota_stale_after_ms: 10_000,
        });

        assert_eq!(state.health, CandidateHealth::Blocked);
        assert!(!state.routing_eligible);
        assert_eq!(
            state.routing_block_reason,
            Some(AccountRoutingBlockReason::SubscriptionForbidden)
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
