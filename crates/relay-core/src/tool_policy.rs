//! Opt-in provider-native tool-catalog optimization.
//!
//! Relay deliberately has only two catalog modes:
//! - `pass_through` keeps the request unchanged;
//! - `automatic` lets a compatible native Responses provider defer function
//!   schemas and choose what to load.
//!
//! Relay does not infer relevance locally, remove tools by name, or turn this
//! setting into an execution permission boundary.
mod catalog;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub(crate) use catalog::catalog_stats;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyMode {
    #[default]
    // Retired name filters are read as standard mode, never applied again.
    #[serde(alias = "standard", alias = "allowlist", alias = "denylist")]
    PassThrough,
    #[serde(alias = "optimized")]
    Automatic,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolPolicy {
    pub mode: ToolPolicyMode,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            mode: ToolPolicyMode::PassThrough,
        }
    }
}

impl<'de> Deserialize<'de> for ToolPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        struct RawToolPolicy {
            #[serde(default)]
            mode: Option<ToolPolicyMode>,
            #[serde(default, rename = "enabledTools")]
            _legacy_enabled_tools: Option<serde::de::IgnoredAny>,
            #[serde(default, rename = "disabledTools")]
            _legacy_disabled_tools: Option<serde::de::IgnoredAny>,
            // Older settings may still contain trigger thresholds. Accept
            // and discard them so the on/off policy remains migration-safe.
            #[serde(default, rename = "automaticToolCountThreshold")]
            _legacy_automatic_tool_count_threshold: Option<serde::de::IgnoredAny>,
            #[serde(default, rename = "automaticSchemaBytesThreshold")]
            _legacy_automatic_schema_bytes_threshold: Option<serde::de::IgnoredAny>,
        }

        let raw = RawToolPolicy::deserialize(deserializer)?;
        let defaults = Self::default();
        Ok(Self {
            mode: raw.mode.unwrap_or(defaults.mode),
        })
    }
}

impl ToolPolicy {
    pub fn normalized(self) -> Result<Self, &'static str> {
        // Retired count/size thresholds are accepted only by the deserializer
        // for compatibility and are not part of the current policy.
        Ok(self)
    }
}

/// Compare-and-set contract shared by desktop IPC and remote management.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolPolicyUpdate {
    pub policy: ToolPolicy,
    pub expected_policy: ToolPolicy,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPolicyOutcome {
    PassThrough,
    BelowThreshold,
    /// Optimization was not applicable to the selected route or client choice.
    Unchanged,
    Deferred,
    /// Historical values remain readable in old usage rows. New requests
    /// never emit name-filtering outcomes.
    #[doc(hidden)]
    NoSelection,
    #[doc(hidden)]
    Filtered,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct CatalogStats {
    pub count: u16,
    /// Compact JSON bytes for catalog arrays, not a token/billing estimate.
    pub bytes: u64,
}

pub(crate) struct ToolPolicyResult {
    pub before: CatalogStats,
    pub after: CatalogStats,
    pub outcome: ToolPolicyOutcome,
}

/// Applies the non-destructive part of the policy. No tool declaration is
/// removed or rewritten here; native Responses deferred loading is enabled at
/// the final route boundary after adapter preparation.
pub(crate) fn apply_tool_policy(
    request: &mut Value,
    policy: &ToolPolicy,
) -> Result<ToolPolicyResult, &'static str> {
    let stats = catalog_stats(request);
    let outcome = match policy.mode {
        ToolPolicyMode::PassThrough => ToolPolicyOutcome::PassThrough,
        ToolPolicyMode::Automatic => ToolPolicyOutcome::Unchanged,
    };
    Ok(ToolPolicyResult {
        before: stats,
        after: stats,
        outcome,
    })
}

/// Enables hosted provider-native tool search for every eligible automatic
/// request. This does not infer relevance from prompt text: the provider/model
/// performs the search while Relay keeps the complete trusted catalog
/// available for loading.
pub(crate) fn enable_deferred_tool_search(request: &mut Value, policy: &ToolPolicy) -> bool {
    if policy.mode != ToolPolicyMode::Automatic {
        return false;
    }
    if catalog::has_deferred_tools(request) {
        return false;
    }
    catalog::enable_deferred_tool_search(request)
}

#[cfg(test)]
mod tests;
