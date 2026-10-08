use super::super::*;
use super::ConfigurationPreset;

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
        let mut serialized_preset = serializer.serialize_struct(
            "ConfigurationPresetSettings",
            6 + usize::from(self.model_reasoning_allowed_levels_present)
                + usize::from(self.model_service_tier_overrides_present)
                + usize::from(self.model_display_order_present),
        )?;
        serialized_preset.serialize_field("sources", &self.sources)?;
        serialized_preset.serialize_field("accounts", &self.accounts)?;
        serialized_preset.serialize_field("routing", &self.routing)?;
        serialized_preset.serialize_field("quota", &self.quota)?;
        serialized_preset.serialize_field("hiddenModels", &self.hidden_models)?;
        serialized_preset.serialize_field("modelPriceOverrides", &self.model_price_overrides)?;
        if self.model_service_tier_overrides_present {
            serialized_preset.serialize_field(
                "modelServiceTierOverrides",
                &self.model_service_tier_overrides,
            )?;
        }
        if self.model_display_order_present {
            serialized_preset.serialize_field("modelDisplayOrder", &self.model_display_order)?;
        }
        if self.model_reasoning_allowed_levels_present {
            serialized_preset.serialize_field(
                "modelReasoningAllowedLevels",
                &self.model_reasoning_allowed_levels,
            )?;
        }
        serialized_preset.end()
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
    /// Accepted only so presets exported by pre-1.1.3 builds remain readable.
    /// Routing is always automatic and the legacy field is never exported.
    #[serde(default, rename = "protocolMode", skip_serializing)]
    pub legacy_protocol_mode: Option<String>,
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

impl SourcePresetRule {
    pub fn apply_resolved_identity(
        &mut self,
        source_id: &str,
        source_name: &str,
        base_url: &str,
        wire_api: WireApi,
    ) {
        self.id = source_id.to_owned();
        self.name = source_name.to_owned();
        self.base_url = base_url.trim_end_matches('/').to_owned();
        self.wire_api = wire_api;
    }
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

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetRoutingPolicy {
    /// Absent in older presets: preserve the destination's current policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_policy: Option<crate::ToolPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool_routing: Option<crate::PoolRoutingPolicy>,
    /// Use the explicitly labelled Excel/Basis Points route for compatible
    /// OAuth accounts. This is a route preference, not a second pool member.
    #[serde(default)]
    pub basis_points_enabled: bool,
    pub max_retry_candidates: u8,
    pub default_service_tier: DefaultServiceTier,
    pub image_base_model: Option<String>,
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
