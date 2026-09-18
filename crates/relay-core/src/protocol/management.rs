use super::Capabilities;
use crate::pricing::{
    ImageRequestPrice, PriceSource, PricingCatalog, PricingContext, PricingMetadata,
    PricingSourceSummary, ResolvedPrice,
};
use crate::{
    automations::{WakeHistory, WakeTask},
    codex_model_display_name, codex_model_is_picker_eligible, normalize_image_base_model,
    normalize_model_ids, normalize_model_price_overrides, normalize_model_reasoning_allowed_levels,
    normalize_model_service_tier_overrides, normalize_source_protocol_bindings,
    normalize_subscription_plan_order, ApiModelPriceOverride, DefaultServiceTier, ModelRules,
    RoutingStrategy, SourceProtocolBinding, TokenPrice, WireApi,
};
mod account;
mod model;
mod model_protocols;
mod routing;
mod usage;

pub use account::{
    api_equivalent_projection_window, model_has_native_account_route, AccountSummary,
    QuotaWindowUsage, RemoteAccountLocation, RevealedAccountIdentity, SourceSummary,
};
pub use model::{
    apply_member_model_display_order, apply_model_display_order,
    apply_model_display_order_with_catalog, apply_model_metadata, apply_model_reasoning_summary,
    apply_model_speed_summary, apply_pool_model_configuration, member_model_catalog,
    model_has_api_source_route, pool_candidate_count, pooled_source_runtime_available,
    source_runtime_available, GatewaySummary, ModelCatalogIdentity, ModelSummary,
};
pub use model_protocols::{
    codex_catalog_supports_websockets, model_protocol_routes, ModelProtocolRoute,
};
pub use routing::{
    account_candidate_enabled, account_operational_state, operational_status, pool_routing_summary,
    quota_refresh_status, AccountOperationalInput, AccountOperationalState,
    AccountRoutingBlockReason, OperationalStatus, ProxyMode, QuotaRefreshStatus,
};
pub use usage::{
    UsageBucket, UsageGroup, UsagePage, UsageQuery, UsageRange, UsageSummary, UsageTokenBreakdown,
    UsageTotals,
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
pub const CONFIGURATION_PRESET_SCHEMA_VERSION: u16 = 4;
const MIN_CONFIGURATION_PRESET_SCHEMA_VERSION: u16 = 2;
const MAX_PRESET_MEMBERS: usize = 2_048;
const MAX_PRESET_MODELS: usize = 4_096;
const MAX_SOURCE_RECOVERY_DELAY_SECONDS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ConfigurationPreset {
    pub format: String,
    pub schema_version: u16,
    pub settings: ConfigurationPresetSettings,
}

/// Validates and canonicalizes a portable configuration preset before either
/// the desktop or server resolves its references to local credentials.
///
/// # Errors
///
/// Returns a redacted validation message when the schema, member references,
/// routing policy, model policy, or endpoint metadata is invalid.
pub fn normalize_configuration_preset(
    mut preset: ConfigurationPreset,
) -> Result<ConfigurationPreset, String> {
    if preset.format != CONFIGURATION_PRESET_FORMAT {
        return Err("configuration preset format is unsupported".into());
    }
    if !(MIN_CONFIGURATION_PRESET_SCHEMA_VERSION..=CONFIGURATION_PRESET_SCHEMA_VERSION)
        .contains(&preset.schema_version)
    {
        return Err(format!(
            "configuration preset schema {} is unsupported",
            preset.schema_version
        ));
    }
    normalize_source_preset_rules(&mut preset.settings.sources)?;
    normalize_account_preset_rules(&mut preset.settings.accounts)?;
    if let Some(policy) = &preset.settings.routing.pool_routing {
        policy.validate().map_err(str::to_string)?;
    }
    preset.settings.routing.subscription_plan_order =
        normalize_subscription_plan_order(preset.settings.routing.subscription_plan_order)
            .map_err(str::to_string)?;
    preset.settings.routing.image_base_model =
        normalize_image_base_model(preset.settings.routing.image_base_model)
            .map_err(|error| error.to_string())?;
    if !(1..=8).contains(&preset.settings.routing.max_retry_candidates)
        || !(1..=8).contains(&preset.settings.routing.cooldown_after_failures)
        || !(10..=20).contains(&preset.settings.quota.request_timeout_seconds)
    {
        return Err("configuration preset policy is invalid".into());
    }
    preset.settings.hidden_models = normalize_preset_model_ids(preset.settings.hidden_models)?;
    preset.settings.model_price_overrides =
        normalize_model_price_overrides(preset.settings.model_price_overrides)
            .map_err(|message| format!("configuration preset {message}"))?;
    preset.settings.model_reasoning_allowed_levels =
        normalize_model_reasoning_allowed_levels(preset.settings.model_reasoning_allowed_levels)
            .map_err(|message| format!("configuration preset {message}"))?;
    preset.settings.model_service_tier_overrides =
        normalize_model_service_tier_overrides(preset.settings.model_service_tier_overrides)
            .map_err(|message| format!("configuration preset {message}"))?;
    preset.settings.model_display_order = normalize_model_ids(preset.settings.model_display_order);
    Ok(preset)
}

/// Applies the portable part of a preset to the matching members in the
/// current configuration. Credentials and endpoint identity stay owned by the
/// desktop or server that resolves those references before calling this.
///
/// # Errors
///
/// Returns a redacted message when a requested source or account does not
/// exist in the current configuration.
pub fn merge_configuration_preset_settings(
    current: &ConfigurationPresetSettings,
    requested: &ConfigurationPresetSettings,
) -> Result<ConfigurationPresetSettings, String> {
    let mut merged = current.clone();
    replace_preset_members(
        &mut merged.sources,
        &requested.sources,
        |rule| &rule.id,
        "source",
    )?;
    replace_preset_members(
        &mut merged.accounts,
        &requested.accounts,
        |rule| &rule.id,
        "account",
    )?;
    merged.routing.clone_from(&requested.routing);
    merged.routing.pool_routing = Some(merged.resolved_pool_routing());
    merged.quota.clone_from(&requested.quota);
    merged.hidden_models.clone_from(&requested.hidden_models);
    merged
        .model_price_overrides
        .clone_from(&requested.model_price_overrides);
    if requested.model_reasoning_allowed_levels_present {
        merged
            .model_reasoning_allowed_levels
            .clone_from(&requested.model_reasoning_allowed_levels);
    }
    if requested.model_service_tier_overrides_present {
        merged
            .model_service_tier_overrides
            .clone_from(&requested.model_service_tier_overrides);
    }
    if requested.model_display_order_present {
        merged
            .model_display_order
            .clone_from(&requested.model_display_order);
    }
    merged.model_reasoning_allowed_levels_present = true;
    merged.model_service_tier_overrides_present = true;
    merged.model_display_order_present = true;
    Ok(merged)
}

/// Verifies that reference resolution did not collapse multiple portable
/// members onto the same local source or account.
///
/// # Errors
///
/// Returns a redacted validation message when multiple portable members resolve
/// to one local member.
pub fn validate_resolved_configuration_preset_members(
    settings: &ConfigurationPresetSettings,
) -> Result<(), String> {
    validate_unique_preset_member_ids(&settings.sources, |rule| &rule.id, "source")?;
    validate_unique_preset_member_ids(&settings.accounts, |rule| &rule.id, "account")?;
    if let Some(policy) = &settings.routing.pool_routing {
        policy.validate().map_err(str::to_string)?;
        if policy.members.iter().any(|member| match member.kind {
            crate::PoolMemberKind::Source => !settings
                .sources
                .iter()
                .any(|m| m.id == member.id && m.in_pool),
            crate::PoolMemberKind::Account => !settings
                .accounts
                .iter()
                .any(|m| m.id == member.id && m.in_pool),
        }) {
            return Err("pool routing references a member outside the preset pool".into());
        }
    }
    Ok(())
}

impl ConfigurationPresetSettings {
    pub fn resolved_pool_routing(&self) -> crate::PoolRoutingPolicy {
        let members = self
            .sources
            .iter()
            .filter(|m| m.in_pool)
            .map(|m| {
                (
                    crate::PoolMemberKind::Source,
                    m.id.clone(),
                    m.priority,
                    m.weight,
                )
            })
            .chain(self.accounts.iter().filter(|m| m.in_pool).map(|m| {
                (
                    crate::PoolMemberKind::Account,
                    m.id.clone(),
                    m.priority,
                    m.weight,
                )
            }))
            .collect();
        crate::resolve_pool_routing(self.routing.pool_routing.as_ref(), members)
    }
}

fn validate_unique_preset_member_ids<T, F>(members: &[T], id: F, kind: &str) -> Result<(), String>
where
    F: Fn(&T) -> &String,
{
    let unique_count = members.iter().map(id).collect::<BTreeSet<_>>().len();
    if unique_count != members.len() {
        return Err(format!(
            "configuration preset resolves multiple {kind} rules to one local {kind}"
        ));
    }
    Ok(())
}

fn replace_preset_members<T, F>(
    current: &mut [T],
    requested: &[T],
    id: F,
    kind: &str,
) -> Result<(), String>
where
    T: Clone,
    F: Fn(&T) -> &String,
{
    let indexes = current
        .iter()
        .enumerate()
        .map(|(index, rule)| (id(rule).clone(), index))
        .collect::<BTreeMap<_, _>>();
    for rule in requested {
        let member_id = id(rule);
        let index = indexes
            .get(member_id)
            .copied()
            .ok_or_else(|| format!("referenced {kind} {member_id} does not exist"))?;
        current[index] = rule.clone();
    }
    Ok(())
}

fn normalize_source_preset_rules(rules: &mut [SourcePresetRule]) -> Result<(), String> {
    if rules.len() > MAX_PRESET_MEMBERS {
        return Err("configuration preset contains too many sources".into());
    }
    let mut ids = BTreeSet::new();
    for rule in rules.iter_mut() {
        validate_preset_reference(&rule.id, "source")?;
        rule.name = rule.name.trim().to_string();
        rule.base_url = rule.base_url.trim().trim_end_matches('/').to_string();
        let valid_url = url::Url::parse(&rule.base_url)
            .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.has_host());
        if !ids.insert(rule.id.clone())
            || rule.weight == 0
            || rule.recovery_delay_seconds > MAX_SOURCE_RECOVERY_DELAY_SECONDS
            || rule.name.is_empty()
            || rule.name.len() > 256
            || rule.name.chars().any(char::is_control)
            || !valid_url
        {
            return Err("configuration preset source rule is invalid".into());
        }
        rule.allowed_models = normalize_preset_model_ids(std::mem::take(&mut rule.allowed_models))?;
        rule.excluded_models =
            normalize_preset_model_ids(std::mem::take(&mut rule.excluded_models))?;
        rule.model_price_overrides =
            normalize_model_price_overrides(std::mem::take(&mut rule.model_price_overrides))
                .map_err(|message| format!("configuration preset {message}"))?;
        if !rule.protocol_bindings.is_empty() {
            rule.protocol_bindings = normalize_source_protocol_bindings(
                std::mem::take(&mut rule.protocol_bindings),
                rule.wire_api,
                &[],
            )
            .map_err(|error| error.to_string())?;
        }
    }
    rules.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn normalize_account_preset_rules(rules: &mut [AccountPresetRule]) -> Result<(), String> {
    if rules.len() > MAX_PRESET_MEMBERS {
        return Err("configuration preset contains too many accounts".into());
    }
    let mut ids = BTreeSet::new();
    for rule in rules.iter_mut() {
        validate_preset_reference(&rule.id, "account")?;
        if !ids.insert(rule.id.clone())
            || rule.weight == 0
            || invalid_preset_reference(&rule.identity_hint)
            || rule
                .proxy_id
                .as_deref()
                .is_some_and(invalid_preset_reference)
            || rule.proxy_id.is_some() && rule.bypass_common_proxy
        {
            return Err("configuration preset account rule is invalid".into());
        }
        rule.allowed_models = normalize_preset_model_ids(std::mem::take(&mut rule.allowed_models))?;
        rule.excluded_models =
            normalize_preset_model_ids(std::mem::take(&mut rule.excluded_models))?;
    }
    rules.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(())
}

fn validate_preset_reference(value: &str, kind: &str) -> Result<(), String> {
    if invalid_preset_reference(value) {
        return Err(format!("configuration preset {kind} reference is invalid"));
    }
    Ok(())
}

fn invalid_preset_reference(value: &str) -> bool {
    value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn normalize_preset_model_ids(models: Vec<String>) -> Result<Vec<String>, String> {
    if models.len() > MAX_PRESET_MODELS {
        return Err("configuration preset model list is too large".into());
    }
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for model in models {
        let model = model.trim();
        if model.is_empty() {
            continue;
        }
        if !crate::is_valid_model_id(model) {
            return Err("configuration preset model id is invalid".into());
        }
        if seen.insert(model.to_ascii_lowercase()) {
            normalized.push(model.to_string());
        }
    }
    Ok(normalized)
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pricing_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub official_provider_family: Option<String>,
    pub wire_api: WireApi,
    #[serde(
        default,
        skip_serializing_if = "crate::ProtocolSelectionMode::is_manual"
    )]
    pub protocol_mode: crate::ProtocolSelectionMode,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_routing: Option<crate::PoolRoutingPolicy>,
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
    #[serde(default)]
    pub pricing: PricingMetadata,
}

#[cfg(test)]
pub fn pool_model_summaries(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    hidden_models: &[String],
) -> Vec<ModelSummary> {
    let catalog =
        PricingCatalog::from_litellm_json(include_str!("../../tests/fixtures/litellm-prices.json"))
            .expect("pricing fixture must be valid");
    pool_model_summaries_with_pricing(
        sources,
        accounts,
        hidden_models,
        &catalog,
        &PricingContext::default(),
    )
}

/// Builds pool model rows from the immutable LiteLLM snapshot and explicit
/// source/account pricing context.
pub fn pool_model_summaries_with_pricing(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    hidden_models: &[String],
    catalog: &PricingCatalog,
    context: &PricingContext,
) -> Vec<ModelSummary> {
    let models = collect_pool_models(sources, accounts);
    let mut summaries = models
        .into_values()
        .map(|model| {
            let id = model.id.clone();
            let resolved = resolve_pool_model_price(&model, &id, catalog, context);
            let quote = resolved.as_ref().and_then(|price| price.quote);
            let enabled = !hidden_models
                .iter()
                .any(|hidden| hidden.eq_ignore_ascii_case(&id));
            (
                model.upstream_order,
                model_summary(
                    id.clone(),
                    model.members.len(),
                    enabled,
                    quote,
                    catalog.image_request_prices(&id),
                ),
            )
        })
        .collect::<Vec<_>>();
    summaries.sort_by_key(|(upstream_order, _)| *upstream_order);
    summaries.into_iter().map(|(_, summary)| summary).collect()
}

/// Returns the provenance represented by the currently eligible pool models.
/// A snapshot can contain several source/account policies, so selecting the
/// first model's source would be misleading; every resolved member contributes
/// to the aggregate and mixed provenance is reported explicitly.
pub fn pool_pricing_source_summary(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    catalog: &PricingCatalog,
    context: &PricingContext,
) -> PricingSourceSummary {
    let mut resolved_sources = Vec::new();
    for model in collect_pool_models(sources, accounts).values() {
        for member in &model.members {
            let Some((kind, candidate_id)) = member.split_once(':') else {
                continue;
            };
            let resolved = context.candidate_price(catalog, kind, candidate_id, Some(&model.id));
            if resolved.quote.is_some() {
                resolved_sources.push(resolved.source);
            } else {
                resolved_sources.push(PriceSource::Unpriced);
            }
        }
    }
    PricingSourceSummary::from_sources(resolved_sources)
}

fn collect_pool_models(
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
) -> BTreeMap<String, PoolModel> {
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

    models
}

fn model_summary(
    id: String,
    member_count: usize,
    enabled: bool,
    quote: Option<TokenPrice>,
    image_request_prices: Vec<ImageRequestPrice>,
) -> ModelSummary {
    let (input, cached, cache_write_5m, cache_write_1h, output) =
        quote.map_or((None, None, None, None, None), |price| {
            (
                Some(price.input),
                price.cache_read,
                price.cache_write_5m,
                price.cache_write_1h,
                Some(price.output),
            )
        });
    ModelSummary {
        enabled,
        protocol_routes: Vec::new(),
        codex_visible: enabled && codex_model_is_picker_eligible(&id),
        codex_display_name: codex_model_display_name(&id),
        id,
        member_count,
        catalog_provider: None,
        catalog_family: None,
        catalog_name: None,
        catalog_release_date: None,
        catalog_last_updated: None,
        catalog_status: None,
        catalog_reasoning: None,
        catalog_reasoning_method: None,
        catalog_reasoning_effort_levels: Vec::new(),
        catalog_default_reasoning_effort: None,
        catalog_tool_call: None,
        catalog_structured_output: None,
        catalog_attachment: None,
        catalog_open_weights: None,
        catalog_input_modalities: Vec::new(),
        catalog_output_modalities: Vec::new(),
        catalog_context_limit: None,
        catalog_input_limit: None,
        catalog_output_limit: None,
        input_micro_usd_per_million: input,
        cached_input_micro_usd_per_million: cached,
        cache_write_5m_micro_usd_per_million: cache_write_5m,
        cache_write_1h_micro_usd_per_million: cache_write_1h,
        output_micro_usd_per_million: output,
        image_request_prices,
        custom_price: false,
        reasoning_levels: Vec::new(),
        reasoning_supported_levels: Vec::new(),
        reasoning_allowed_levels: Vec::new(),
        reasoning_configurable: false,
        reasoning_manual_fallback: false,
        speed_supported: false,
        speed_tiers: Vec::new(),
        speed_tier: DefaultServiceTier::Standard,
        speed_configurable: false,
    }
}

fn resolve_pool_model_price(
    model: &PoolModel,
    model_id: &str,
    catalog: &PricingCatalog,
    context: &PricingContext,
) -> Option<ResolvedPrice> {
    let mut fallback = None;
    let mut resolved: Option<ResolvedPrice> = None;
    for member in &model.members {
        let Some((kind, candidate_id)) = member.split_once(':') else {
            continue;
        };
        let candidate = context.candidate_price(catalog, kind, candidate_id, Some(model_id));
        if let Some(candidate_quote) = candidate.quote {
            if let Some(mut current) = resolved {
                let current_quote = current
                    .quote
                    .expect("a resolved pool price always has a quote");
                current.quote = Some(TokenPrice {
                    input: current_quote.input,
                    cache_read: current_quote.cache_read,
                    // The same public model can be exposed by a generic route
                    // and an Anthropic Messages route. Preserve the primary
                    // route's price while filling cache-write fields only
                    // from the route-aware Messages evidence.
                    cache_write_5m: current_quote
                        .cache_write_5m
                        .or(candidate_quote.cache_write_5m),
                    cache_write_1h: current_quote
                        .cache_write_1h
                        .or(candidate_quote.cache_write_1h),
                    output: current_quote.output,
                });
                resolved = Some(current);
            } else {
                resolved = Some(candidate);
            }
        } else {
            fallback = Some(candidate);
        }
    }
    resolved.or(fallback)
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
        model_metadata::ModelMetadataCatalog,
        quota::{QuotaSnapshot, QuotaWindow, QuotaWindowKind, Subscription, SubscriptionStatus},
        CandidateHealth, CandidateQuota,
    };
    use crate::{
        ActiveModelRuntime, ApiEquivalentSummary, CandidateKind, CandidateRuntimeSnapshot,
        GatewayRuntime, GatewayRuntimeOptions, LocalGatewayKey, MessagesReasoningMode,
        PriceEvidence, ProviderSource, RuntimeLocalKey, RuntimeSource, SourceAdapter,
        SourcePricingMetadata,
    };
    use std::sync::Arc;

    fn runtime_candidate(
        candidate_id: &str,
        kind: CandidateKind,
        available: bool,
    ) -> CandidateRuntimeSnapshot {
        CandidateRuntimeSnapshot {
            candidate_id: candidate_id.into(),
            kind,
            available,
            next_for_new_request: false,
            activity_revision: 0,
            runtime_id: 0,
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
            provider_family: None,
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
            client_auth_status: None,
            last_client_login_redirect_at_ms: None,
        }
    }

    fn source_summary(id: &str, models: &[&str]) -> SourceSummary {
        SourceSummary {
            resolved_protocol_bindings: None,
            id: id.into(),
            name: id.into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            protocol_config: crate::SourceProtocolConfig::default(),
            models: models.iter().map(ToString::to_string).collect(),
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
        }
    }

    fn test_token_price(input: u64, output: u64) -> TokenPrice {
        TokenPrice {
            input,
            cache_read: Some(input / 10),
            cache_write_5m: None,
            cache_write_1h: None,
            output,
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
    fn model_reasoning_summary_does_not_invent_unsupported_manual_levels() {
        let mut model = ModelSummary {
            enabled: true,
            protocol_routes: Vec::new(),
            codex_visible: true,
            codex_display_name: String::new(),
            id: "gpt-test".into(),
            member_count: 1,
            catalog_provider: None,
            catalog_family: None,
            catalog_name: None,
            catalog_release_date: None,
            catalog_last_updated: None,
            catalog_status: None,
            catalog_reasoning: None,
            catalog_reasoning_method: None,
            catalog_reasoning_effort_levels: Vec::new(),
            catalog_default_reasoning_effort: None,
            catalog_tool_call: None,
            catalog_structured_output: None,
            catalog_attachment: None,
            catalog_open_weights: None,
            catalog_input_modalities: Vec::new(),
            catalog_output_modalities: Vec::new(),
            catalog_context_limit: None,
            catalog_input_limit: None,
            catalog_output_limit: None,
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
            reasoning_manual_fallback: false,
            speed_supported: false,
            speed_tiers: Vec::new(),
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
        assert!(!model.reasoning_manual_fallback);

        apply_model_reasoning_summary(&mut model, None, None, false);
        assert!(model.reasoning_levels.is_empty());
        assert!(model.reasoning_supported_levels.is_empty());
        assert!(model.reasoning_allowed_levels.is_empty());
        assert!(!model.reasoning_configurable);
        assert!(!model.reasoning_manual_fallback);

        apply_model_reasoning_summary(&mut model, Some(vec!["high".into()]), None, true);
        assert_eq!(model.reasoning_levels, ["high"]);
        assert_eq!(model.reasoning_supported_levels, ["high"]);
        assert_eq!(model.reasoning_allowed_levels, ["high"]);
        assert!(model.reasoning_configurable);
        assert!(!model.reasoning_manual_fallback);

        apply_model_reasoning_summary(&mut model, Some(Vec::new()), Some(&["max".into()]), true);
        assert!(model.reasoning_levels.is_empty());
        assert!(model.reasoning_supported_levels.is_empty());
        assert!(model.reasoning_allowed_levels.is_empty());
        assert!(!model.reasoning_configurable);
        assert!(!model.reasoning_manual_fallback);

        model.id = "gpt-5.6-terra".into();
        apply_model_reasoning_summary(&mut model, Some(Vec::new()), None, true);
        assert!(model.reasoning_supported_levels.is_empty());
        assert!(model.reasoning_allowed_levels.is_empty());
        assert!(!model.reasoning_manual_fallback);

        apply_model_reasoning_summary(&mut model, Some(vec!["ultra".into()]), None, true);
        assert_eq!(model.reasoning_supported_levels, ["ultra"]);
        assert_eq!(model.reasoning_allowed_levels, ["ultra"]);
        assert!(!model.reasoning_manual_fallback);

        model.id = "claude-fable-5-1".into();
        apply_model_reasoning_summary(&mut model, None, None, true);
        assert!(model.reasoning_supported_levels.is_empty());
        assert!(model.reasoning_allowed_levels.is_empty());
        assert!(!model.reasoning_configurable);
        assert!(!model.reasoning_manual_fallback);
    }

    #[test]
    fn anthropic_modes_are_limited_to_provider_reported_levels() {
        let mut model = ModelSummary {
            enabled: true,
            protocol_routes: Vec::new(),
            codex_visible: true,
            codex_display_name: String::new(),
            id: "claude-opus-4-8".into(),
            member_count: 1,
            catalog_provider: None,
            catalog_family: None,
            catalog_name: None,
            catalog_release_date: None,
            catalog_last_updated: None,
            catalog_status: None,
            catalog_reasoning: None,
            catalog_reasoning_method: None,
            catalog_reasoning_effort_levels: Vec::new(),
            catalog_default_reasoning_effort: None,
            catalog_tool_call: None,
            catalog_structured_output: None,
            catalog_attachment: None,
            catalog_open_weights: None,
            catalog_input_modalities: Vec::new(),
            catalog_output_modalities: Vec::new(),
            catalog_context_limit: None,
            catalog_input_limit: None,
            catalog_output_limit: None,
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
            reasoning_manual_fallback: false,
            speed_supported: false,
            speed_tiers: Vec::new(),
            speed_tier: DefaultServiceTier::Standard,
            speed_configurable: false,
        };
        apply_model_reasoning_summary(
            &mut model,
            Some(vec!["low".into(), "max".into()]),
            None,
            true,
        );
        assert_eq!(model.reasoning_supported_levels, ["low", "max"]);
        assert_eq!(model.reasoning_levels, ["low", "max"]);
    }

    #[test]
    fn api_source_reasoning_route_requires_an_active_responses_source() {
        let source = SourceSummary {
            resolved_protocol_bindings: None,
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            protocol_config: crate::SourceProtocolConfig::default(),
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
    fn model_summaries_apply_member_rules_hidden_state_and_discovery_order() {
        let source = SourceSummary {
            resolved_protocol_bindings: None,
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            protocol_config: crate::SourceProtocolConfig::default(),
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
        assert!(models[2].output_micro_usd_per_million.is_none());
    }

    #[test]
    fn member_model_order_uses_complete_inventory_independently_of_rules() {
        let metadata = ModelMetadataCatalog::from_models_dev_json(
            r#"{
                "test/newer":{"release_date":"2026-01-01"},
                "test/older":{"release_date":"2025-01-01"},
                "test/catalog-only":{"release_date":"2026-02-01"}
            }"#,
        )
        .unwrap();
        let inventory = ["unknown-z", "older", "newer", "unknown-a"];
        let mut sources = vec![source_summary("source", &inventory)];
        let mut accounts = vec![account_summary(false, &inventory)];
        sources[0].excluded_models = vec!["newer".into()];
        sources[0].enabled = false;
        accounts[0].allowed_models = vec!["older".into()];
        let original_source = sources[0].clone();
        let original_account = accounts[0].clone();

        apply_member_model_display_order(&mut sources, &mut accounts, &[], &metadata);
        let expected = ["newer", "older", "unknown-z", "unknown-a"];
        assert_eq!(sources[0].models, expected);
        assert_eq!(accounts[0].models, expected);
        let mut expected_source = original_source;
        expected_source.models = expected.iter().map(ToString::to_string).collect();
        let mut expected_account = original_account;
        expected_account.models = expected_source.models.clone();
        assert_eq!(sources[0], expected_source);
        assert_eq!(accounts[0], expected_account);
        let identities = member_model_catalog(&sources, &accounts, &metadata);
        assert_eq!(identities.len(), 2);
        assert_eq!(identities["newer"].catalog_provider, "test");
        assert_eq!(identities["older"].catalog_provider, "test");
        assert!(!identities.contains_key("unknown-z"));
        assert!(!identities.contains_key("catalog-only"));

        sources[0].excluded_models.clear();
        accounts[0].excluded_models = vec!["older".into()];
        apply_member_model_display_order(&mut sources, &mut accounts, &[], &metadata);
        assert_eq!(sources[0].models, expected);
        assert_eq!(accounts[0].models, expected);

        apply_member_model_display_order(
            &mut sources,
            &mut accounts,
            &["stale".into(), "older".into(), "NEWER".into()],
            &metadata,
        );
        assert_eq!(
            sources[0].models,
            ["older", "newer", "unknown-z", "unknown-a"]
        );
        assert_eq!(accounts[0].models, sources[0].models);
    }

    #[test]
    fn advisory_metadata_changes_only_presentation_fields_and_order() {
        let source = source_summary("source", &["older", "newer"]);
        let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
        let original_state = models
            .iter()
            .map(|model| {
                (
                    model.id.clone(),
                    (
                        model.enabled,
                        model.member_count,
                        model.input_micro_usd_per_million,
                        model.output_micro_usd_per_million,
                    ),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let metadata = ModelMetadataCatalog::from_models_dev_json(
            r#"{
                "test/newer":{
                    "name":"Newer",
                    "family":"test",
                    "release_date":"2026-01-01",
                    "reasoning":true,
                    "reasoning_effort_levels":["low","high"],
                    "default_reasoning_effort":"low",
                    "tool_call":true,
                    "structured_output":true,
                    "attachment":true,
                    "open_weights":false,
                    "modalities":{"input":["text","image"],"output":["text"]},
                    "limit":{"context":128000,"input":120000,"output":8000}
                },
                "test/older":{"name":"Older","family":"test","release_date":"2025-01-01"}
            }"#,
        )
        .unwrap();

        apply_model_metadata(&mut models, &metadata);
        apply_model_display_order_with_catalog(&mut models, &[], &metadata);

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
        assert_eq!(models[0].catalog_name.as_deref(), Some("Newer"));
        assert_eq!(models[0].catalog_reasoning, Some(true));
        assert_eq!(models[0].catalog_reasoning_effort_levels, ["low", "high"]);
        assert_eq!(
            models[0].catalog_default_reasoning_effort.as_deref(),
            Some("low")
        );
        assert_eq!(models[0].catalog_tool_call, Some(true));
        assert_eq!(models[0].catalog_structured_output, Some(true));
        assert_eq!(models[0].catalog_attachment, Some(true));
        assert_eq!(models[0].catalog_open_weights, Some(false));
        assert_eq!(models[0].catalog_input_modalities, ["text", "image"]);
        assert_eq!(models[0].catalog_output_modalities, ["text"]);
        assert_eq!(models[0].catalog_context_limit, Some(128_000));
        assert_eq!(models[0].catalog_input_limit, Some(120_000));
        assert_eq!(models[0].catalog_output_limit, Some(8_000));
        assert_eq!(
            models
                .iter()
                .map(|model| {
                    (
                        model.id.clone(),
                        (
                            model.enabled,
                            model.member_count,
                            model.input_micro_usd_per_million,
                            model.output_micro_usd_per_million,
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>(),
            original_state
        );
    }

    #[test]
    fn refreshing_metadata_clears_removed_presentation_fields() {
        let source = source_summary("source", &["test/model"]);
        let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
        let metadata = ModelMetadataCatalog::from_models_dev_json(
            r#"{"test/model":{"name":"Model","family":"test","status":"active"}}"#,
        )
        .unwrap();

        apply_model_metadata(&mut models, &metadata);
        assert_eq!(models[0].catalog_name.as_deref(), Some("Model"));
        assert_eq!(models[0].catalog_provider.as_deref(), Some("test"));

        apply_model_metadata(&mut models, &ModelMetadataCatalog::empty());

        assert_eq!(models[0].catalog_provider, None);
        assert_eq!(models[0].catalog_family, None);
        assert_eq!(models[0].catalog_name, None);
        assert_eq!(models[0].catalog_release_date, None);
        assert_eq!(models[0].catalog_last_updated, None);
        assert_eq!(models[0].catalog_status, None);
    }

    #[test]
    fn pool_pricing_summary_reports_provider_evidence() {
        let source = source_summary("provider", &["gpt-test"]);
        let price = test_token_price(1_000_000, 2_000_000);
        let context = PricingContext {
            source_evidence: BTreeMap::from([(
                "provider".into(),
                BTreeMap::from([(
                    "gpt-test".into(),
                    PriceEvidence {
                        provider: Some(price),
                        manual: None,
                    },
                )]),
            )]),
            ..Default::default()
        };

        assert_eq!(
            pool_pricing_source_summary(&[source], &[], &PricingCatalog::empty(), &context),
            PricingSourceSummary::Provider
        );
    }

    #[test]
    fn pool_model_price_keeps_messages_cache_creation_when_generic_route_wins_order() {
        let generic = source_summary("a-generic", &["claude-test"]);
        let mut messages = source_summary("z-messages", &["claude-test"]);
        messages.wire_api = WireApi::Messages;
        messages.protocol_bindings = vec![SourceProtocolBinding {
            wire_api: WireApi::Messages,
            adapter: SourceAdapter::Native,
            reasoning_mode: MessagesReasoningMode::Disabled,
            cache_write_ttl: Default::default(),
            model_ids: vec!["claude-test".into()],
        }];
        let generic_price = TokenPrice {
            input: 1_000_000,
            cache_read: Some(100_000),
            cache_write_5m: None,
            cache_write_1h: None,
            output: 2_000_000,
        };
        let messages_price = TokenPrice {
            cache_write_5m: Some(1_250_000),
            cache_write_1h: Some(2_500_000),
            ..generic_price
        };
        let context = PricingContext {
            source_metadata: BTreeMap::from([
                (
                    "a-generic".into(),
                    SourcePricingMetadata {
                        cache_write_models: BTreeSet::new(),
                        ..Default::default()
                    },
                ),
                (
                    "z-messages".into(),
                    SourcePricingMetadata {
                        cache_write_models: BTreeSet::from(["claude-test".into()]),
                        ..Default::default()
                    },
                ),
            ]),
            source_evidence: BTreeMap::from([
                (
                    "a-generic".into(),
                    BTreeMap::from([(
                        "claude-test".into(),
                        PriceEvidence {
                            provider: Some(generic_price),
                            manual: None,
                        },
                    )]),
                ),
                (
                    "z-messages".into(),
                    BTreeMap::from([(
                        "claude-test".into(),
                        PriceEvidence {
                            provider: Some(messages_price),
                            manual: None,
                        },
                    )]),
                ),
            ]),
            ..Default::default()
        };

        let models = pool_model_summaries_with_pricing(
            &[generic, messages],
            &[],
            &[],
            &PricingCatalog::empty(),
            &context,
        );

        assert_eq!(models.len(), 1);
        assert_eq!(
            models[0].cache_write_5m_micro_usd_per_million,
            Some(1_250_000)
        );
        assert_eq!(
            models[0].cache_write_1h_micro_usd_per_million,
            Some(2_500_000)
        );
    }

    #[test]
    fn pool_pricing_summary_reports_manual_evidence_when_catalog_is_unpriced() {
        let source = source_summary("manual", &["private-model"]);
        let price = test_token_price(3_000_000, 4_000_000);
        let context = PricingContext {
            source_evidence: BTreeMap::from([(
                "manual".into(),
                BTreeMap::from([(
                    "private-model".into(),
                    PriceEvidence {
                        provider: None,
                        manual: Some(price),
                    },
                )]),
            )]),
            ..Default::default()
        };

        assert_eq!(
            pool_pricing_source_summary(&[source], &[], &PricingCatalog::empty(), &context),
            PricingSourceSummary::Manual
        );
    }

    #[test]
    fn pool_pricing_summary_reports_mixed_provenance_and_unpriced_pool() {
        let provider = source_summary("provider", &["gpt-test"]);
        let manual = source_summary("manual", &["private-model"]);
        let unknown = source_summary("unknown", &["future-model"]);
        let context = PricingContext {
            source_evidence: BTreeMap::from([
                (
                    "provider".into(),
                    BTreeMap::from([(
                        "gpt-test".into(),
                        PriceEvidence {
                            provider: Some(test_token_price(1_000_000, 2_000_000)),
                            manual: None,
                        },
                    )]),
                ),
                (
                    "manual".into(),
                    BTreeMap::from([(
                        "private-model".into(),
                        PriceEvidence {
                            provider: None,
                            manual: Some(test_token_price(3_000_000, 4_000_000)),
                        },
                    )]),
                ),
            ]),
            ..Default::default()
        };
        let catalog = PricingCatalog::empty();

        assert_eq!(
            pool_pricing_source_summary(
                &[provider.clone(), manual.clone()],
                &[],
                &catalog,
                &context,
            ),
            PricingSourceSummary::Mixed
        );
        assert_eq!(
            pool_pricing_source_summary(&[unknown], &[], &catalog, &context),
            PricingSourceSummary::Unpriced
        );
    }

    #[test]
    fn pool_pricing_summary_ignores_non_eligible_members() {
        let eligible = source_summary("eligible", &["gpt-test"]);
        let mut disabled = source_summary("disabled", &["disabled-model"]);
        disabled.enabled = false;
        let mut outside_pool = source_summary("outside", &["outside-model"]);
        outside_pool.in_pool = false;
        let mut draining = source_summary("draining", &["draining-model"]);
        draining.draining = true;
        let mut missing_secret = source_summary("missing-secret", &["missing-model"]);
        missing_secret.secret_available = false;
        let context = PricingContext {
            source_evidence: BTreeMap::from([(
                "eligible".into(),
                BTreeMap::from([(
                    "gpt-test".into(),
                    PriceEvidence {
                        provider: Some(test_token_price(1_000_000, 2_000_000)),
                        manual: None,
                    },
                )]),
            )]),
            ..Default::default()
        };

        assert_eq!(
            pool_pricing_source_summary(
                &[eligible, disabled, outside_pool, draining, missing_secret],
                &[],
                &PricingCatalog::empty(),
                &context,
            ),
            PricingSourceSummary::Provider
        );
    }

    #[test]
    fn legacy_preset_without_pricing_identity_round_trips_without_new_fields() {
        let mut settings = ConfigurationPresetSettings {
            sources: vec![SourcePresetRule {
                protocol_mode: crate::ProtocolSelectionMode::Manual,
                id: "source".into(),
                name: "Source".into(),
                base_url: "https://example.test/v1".into(),
                pricing_provider: None,
                official_provider_family: None,
                wire_api: WireApi::Responses,
                protocol_bindings: Vec::new(),
                enabled: true,
                in_pool: true,
                allowed_models: Vec::new(),
                excluded_models: Vec::new(),
                priority: 0,
                weight: 1,
                recovery_delay_seconds: 0,
                model_price_overrides: BTreeMap::new(),
            }],
            accounts: Vec::new(),
            routing: PresetRoutingPolicy {
                pool_routing: None,
                max_retry_candidates: 3,
                cooldown_after_failures: 3,
                keep_last_candidate_available: true,
                routing_strategy: RoutingStrategy::Adaptive,
                subscription_plan_order: Vec::new(),
                default_service_tier: DefaultServiceTier::Standard,
                image_base_model: None,
            },
            quota: PresetQuotaPolicy {
                request_timeout_seconds: 30,
                account_proxy_required: false,
                common_proxy_id: None,
            },
            hidden_models: Vec::new(),
            model_price_overrides: BTreeMap::new(),
            model_reasoning_allowed_levels: BTreeMap::new(),
            model_service_tier_overrides: BTreeMap::new(),
            model_display_order: Vec::new(),
            model_reasoning_allowed_levels_present: true,
            model_service_tier_overrides_present: true,
            model_display_order_present: true,
        };
        let mut legacy = serde_json::to_value(ConfigurationPreset {
            format: CONFIGURATION_PRESET_FORMAT.into(),
            schema_version: CONFIGURATION_PRESET_SCHEMA_VERSION,
            settings: settings.clone(),
        })
        .unwrap();
        let source = legacy["settings"]["sources"][0].as_object_mut().unwrap();
        source.remove("pricingProvider");
        source.remove("officialProviderFamily");

        let decoded: ConfigurationPreset = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.settings.sources[0].pricing_provider, None);
        assert_eq!(decoded.settings.sources[0].official_provider_family, None);
        settings.sources[0].pricing_provider = None;
        settings.sources[0].official_provider_family = None;
        assert_eq!(decoded.settings.sources, settings.sources);

        let encoded = serde_json::to_value(decoded).unwrap();
        let source = encoded["settings"]["sources"][0].as_object().unwrap();
        assert!(!source.contains_key("pricingProvider"));
        assert!(!source.contains_key("officialProviderFamily"));
    }

    fn valid_configuration_preset() -> ConfigurationPreset {
        serde_json::from_value(serde_json::json!({
            "format": CONFIGURATION_PRESET_FORMAT,
            "schemaVersion": CONFIGURATION_PRESET_SCHEMA_VERSION,
            "settings": {
                "sources": [{
                    "id": "source_1", "name": "Source", "baseUrl": "https://example.test/v1",
                    "wireApi": "responses", "enabled": true, "inPool": true,
                    "allowedModels": [], "excludedModels": [], "priority": 0, "weight": 1
                }],
                "accounts": [],
                "routing": {
                    "maxRetryCandidates": 3, "cooldownAfterFailures": 3,
                    "keepLastCandidateAvailable": true, "routingStrategy": "adaptive",
                    "subscriptionPlanOrder": [], "defaultServiceTier": "standard", "imageBaseModel": null
                },
                "quota": { "requestTimeoutSeconds": 20, "accountProxyRequired": false, "commonProxyId": null },
                "hiddenModels": [], "modelPriceOverrides": {}, "modelReasoningAllowedLevels": {},
                "modelServiceTierOverrides": {}, "modelDisplayOrder": []
            }
        }))
        .expect("static configuration preset is valid")
    }

    #[test]
    fn configuration_preset_validation_rejects_untrusted_identity_and_endpoint() {
        let mut preset = valid_configuration_preset();
        preset.format = "other-product".into();
        assert!(normalize_configuration_preset(preset).is_err());

        let mut preset = valid_configuration_preset();
        preset.schema_version = CONFIGURATION_PRESET_SCHEMA_VERSION + 1;
        assert!(normalize_configuration_preset(preset).is_err());

        let mut preset = valid_configuration_preset();
        preset.settings.sources[0].base_url = "file:///not-an-api".into();
        assert!(normalize_configuration_preset(preset).is_err());
    }

    #[test]
    fn configuration_preset_validation_normalizes_source_policy() {
        let mut preset = valid_configuration_preset();
        let source = &mut preset.settings.sources[0];
        source.base_url = " https://example.test/v1/ ".into();
        source.allowed_models = vec!["gpt-test".into(), "GPT-TEST".into()];

        let normalized = normalize_configuration_preset(preset).unwrap();
        assert_eq!(
            normalized.settings.sources[0].base_url,
            "https://example.test/v1"
        );
        assert_eq!(normalized.settings.sources[0].allowed_models, ["gpt-test"]);
    }

    #[test]
    fn sparse_configuration_preset_keeps_newer_model_policy() {
        let current = valid_configuration_preset().settings;
        let mut current = ConfigurationPresetSettings {
            model_reasoning_allowed_levels: BTreeMap::from([(
                "gpt-test".into(),
                vec!["high".into()],
            )]),
            model_service_tier_overrides: BTreeMap::from([(
                "gpt-test".into(),
                DefaultServiceTier::Fast,
            )]),
            model_display_order: vec!["gpt-test".into()],
            ..current
        };
        current.model_reasoning_allowed_levels_present = true;
        current.model_service_tier_overrides_present = true;
        current.model_display_order_present = true;
        let mut sparse = current.clone();
        sparse.model_reasoning_allowed_levels.clear();
        sparse.model_service_tier_overrides.clear();
        sparse.model_display_order.clear();
        sparse.model_reasoning_allowed_levels_present = false;
        sparse.model_service_tier_overrides_present = false;
        sparse.model_display_order_present = false;

        let merged = merge_configuration_preset_settings(&current, &sparse).unwrap();

        assert_eq!(
            merged.model_reasoning_allowed_levels,
            current.model_reasoning_allowed_levels
        );
        assert_eq!(
            merged.model_service_tier_overrides,
            current.model_service_tier_overrides
        );
        assert_eq!(merged.model_display_order, current.model_display_order);
    }

    #[test]
    fn resolved_configuration_preset_rejects_duplicate_local_members() {
        let mut settings = valid_configuration_preset().settings;
        settings.sources.push(settings.sources[0].clone());

        assert!(validate_resolved_configuration_preset_members(&settings).is_err());
    }

    #[test]
    fn pool_snapshot_configuration_hides_speed_without_runtime_evidence() {
        let source = SourceSummary {
            resolved_protocol_bindings: None,
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            protocol_config: crate::SourceProtocolConfig::default(),
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
        let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
        let price_overrides = BTreeMap::from([(
            "gpt-test".to_string(),
            ApiModelPriceOverride {
                input_micro_usd_per_million: 12,
                cached_input_micro_usd_per_million: None,
                cache_write_5m_micro_usd_per_million: Some(18),
                cache_write_1h_micro_usd_per_million: Some(9),
                output_micro_usd_per_million: 34,
            },
        )]);
        let reasoning_allowed_levels =
            BTreeMap::from([("gpt-test".to_string(), vec!["high".to_string()])]);
        let service_tier_overrides =
            BTreeMap::from([("gpt-test".to_string(), DefaultServiceTier::Fast)]);

        apply_pool_model_configuration(
            &mut models,
            std::slice::from_ref(&source),
            &[],
            &price_overrides,
            &reasoning_allowed_levels,
            &service_tier_overrides,
            None,
        );

        assert_eq!(models.len(), 1);
        let model = &models[0];
        assert!(model.custom_price);
        assert_eq!(model.input_micro_usd_per_million, Some(12));
        assert_eq!(model.cached_input_micro_usd_per_million, None);
        assert_eq!(model.cache_write_5m_micro_usd_per_million, None);
        assert_eq!(model.cache_write_1h_micro_usd_per_million, None);
        assert_eq!(model.output_micro_usd_per_million, Some(34));
        assert!(model.reasoning_levels.is_empty());
        assert_eq!(model.speed_tier, DefaultServiceTier::Standard);
        assert!(!model.speed_supported);
        assert!(!model.speed_configurable);
        assert_eq!(pool_candidate_count(&[source], &[]), 1);
    }

    #[test]
    fn pool_snapshot_does_not_infer_speed_from_a_runtime_default() {
        let source = SourceSummary {
            resolved_protocol_bindings: None,
            id: "source_1".into(),
            name: "Synthetic".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            protocol_config: crate::SourceProtocolConfig::default(),
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
        let runtime = GatewayRuntime::from_pool(
            vec![RuntimeSource::unrestricted(ProviderSource {
                id: source.id.clone(),
                name: source.name.clone(),
                base_url: source.base_url.clone(),
                api_key: "synthetic-upstream-secret".into(),
                wire_api: source.wire_api,
                models: source.models.clone(),
            })],
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key_1".into(),
                secret: "synthetic-local-secret".into(),
            })],
            GatewayRuntimeOptions {
                default_service_tier: DefaultServiceTier::Fast,
                ..Default::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap();
        let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);

        apply_pool_model_configuration(
            &mut models,
            std::slice::from_ref(&source),
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::new(),
            Some(&runtime),
        );

        assert_eq!(models[0].speed_tier, DefaultServiceTier::Standard);
        assert!(!models[0].speed_supported);
        assert!(!models[0].speed_configurable);
    }

    #[test]
    fn pool_model_summaries_include_the_runtime_messages_bridge() {
        let source = SourceSummary {
            resolved_protocol_bindings: None,
            id: "source_1".into(),
            name: "Mixed source".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_config: crate::SourceProtocolConfig::default(),
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

        let mut models = pool_model_summaries(std::slice::from_ref(&source), &[], &[]);
        apply_pool_model_configuration(
            &mut models,
            std::slice::from_ref(&source),
            &[],
            &BTreeMap::new(),
            &BTreeMap::new(),
            &BTreeMap::from([("claude-native".to_string(), DefaultServiceTier::Fast)]),
            None,
        );

        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            ["gpt-routed", "claude-native"]
        );
        assert!(!models[0].speed_supported);
        assert!(!models[1].speed_supported);
        assert_eq!(models[1].speed_tier, DefaultServiceTier::Standard);
    }

    #[test]
    fn source_summary_preserves_legacy_and_native_protocol_model_boundaries() {
        let legacy = SourceSummary {
            resolved_protocol_bindings: None,
            id: "legacy".into(),
            name: "Legacy".into(),
            enabled: true,
            in_pool: true,
            draining: false,
            operational_status: OperationalStatus::Rotation,
            base_url: "https://example.test/v1".into(),
            pricing_provider: None,
            official_provider_family: None,
            wire_api: WireApi::Responses,
            protocol_bindings: Vec::new(),
            protocol_config: crate::SourceProtocolConfig::default(),
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

        assert_eq!(summary.tokens.reasoning_tokens, None);
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
