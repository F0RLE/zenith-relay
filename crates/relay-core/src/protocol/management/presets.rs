use super::*;

mod rules;

pub const CONFIGURATION_PRESET_FORMAT: &str = "zenith-relay-configuration";
pub const CONFIGURATION_PRESET_SCHEMA_VERSION: u16 = 6;
const MIN_CONFIGURATION_PRESET_SCHEMA_VERSION: u16 = 2;
const MAX_PRESET_MEMBERS: usize = 2_048;

pub const DEFAULT_MAX_RETRY_CANDIDATES: u8 = 3;
pub const MIN_MAX_RETRY_CANDIDATES: u8 = 1;
pub const MAX_MAX_RETRY_CANDIDATES: u8 = 8;
pub const DEFAULT_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;
pub const MIN_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 10;
pub const MAX_QUOTA_REQUEST_TIMEOUT_SECONDS: u64 = 20;

pub fn max_retry_candidates_in_range(candidate_count: u8) -> bool {
    (MIN_MAX_RETRY_CANDIDATES..=MAX_MAX_RETRY_CANDIDATES).contains(&candidate_count)
}

pub fn quota_request_timeout_in_range(seconds: u64) -> bool {
    (MIN_QUOTA_REQUEST_TIMEOUT_SECONDS..=MAX_QUOTA_REQUEST_TIMEOUT_SECONDS).contains(&seconds)
}

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
    rules::normalize_source_preset_rules(&mut preset.settings.sources)?;
    rules::normalize_account_preset_rules(&mut preset.settings.accounts)?;
    preset.settings.routing.tool_policy = preset
        .settings
        .routing
        .tool_policy
        .map(crate::ToolPolicy::normalized)
        .transpose()
        .map_err(str::to_string)?;
    if let Some(policy) = &preset.settings.routing.pool_routing {
        policy.validate().map_err(str::to_string)?;
    }
    preset.settings.routing.image_base_model =
        normalize_image_base_model(preset.settings.routing.image_base_model)
            .map_err(|error| error.to_string())?;
    if !max_retry_candidates_in_range(preset.settings.routing.max_retry_candidates)
        || !quota_request_timeout_in_range(preset.settings.quota.request_timeout_seconds)
    {
        return Err("configuration preset policy is invalid".into());
    }
    preset.settings.hidden_models =
        rules::normalize_preset_model_ids(preset.settings.hidden_models)?;
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
    existing_settings: &ConfigurationPresetSettings,
    requested: &ConfigurationPresetSettings,
) -> Result<ConfigurationPresetSettings, String> {
    let mut merged = existing_settings.clone();
    rules::replace_preset_members(
        &mut merged.sources,
        &requested.sources,
        |rule| &rule.id,
        "source",
    )?;
    rules::replace_preset_members(
        &mut merged.accounts,
        &requested.accounts,
        |rule| &rule.id,
        "account",
    )?;
    let tool_policy = requested
        .routing
        .tool_policy
        .clone()
        .or_else(|| existing_settings.routing.tool_policy.clone());
    merged.routing.clone_from(&requested.routing);
    merged.routing.tool_policy = tool_policy;
    // Older presets use the same forward-only compatibility conversion as
    // persisted profiles. Omission preserves the destination's current order.
    if requested.routing.pool_routing.is_none() {
        merged.routing.pool_routing = existing_settings.routing.pool_routing.clone();
    }
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
    rules::validate_unique_preset_member_ids(&settings.sources, |rule| &rule.id, "source")?;
    rules::validate_unique_preset_member_ids(&settings.accounts, |rule| &rule.id, "account")?;
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

mod types;

pub use types::{
    AccountPresetRule, ConfigurationPresetApplyInput, ConfigurationPresetApplyResult,
    ConfigurationPresetChange, ConfigurationPresetDocument, ConfigurationPresetPreview,
    ConfigurationPresetPreviewInput, ConfigurationPresetSettings, PresetQuotaPolicy,
    PresetRoutingPolicy, SourcePresetRule,
};

#[cfg(test)]
mod range_tests {
    use super::{max_retry_candidates_in_range, quota_request_timeout_in_range};

    #[test]
    fn gateway_policy_bounds_stay_closed() {
        assert!(max_retry_candidates_in_range(1));
        assert!(max_retry_candidates_in_range(8));
        assert!(!max_retry_candidates_in_range(0));
        assert!(!max_retry_candidates_in_range(9));
        assert!(quota_request_timeout_in_range(10));
        assert!(quota_request_timeout_in_range(20));
        assert!(!quota_request_timeout_in_range(9));
        assert!(!quota_request_timeout_in_range(21));
    }
}
