//! Read the old portable preset shape without reviving its V1 settings.

use super::PresetRoutingPolicy;
use crate::DefaultServiceTier;
use serde::{Deserialize, Deserializer};

// Only the four known V1 no-op fields are accepted. Other unknown fields
// remain errors, and none of these compatibility values are exported again.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PresetRoutingPolicyReader {
    #[serde(default)]
    tool_policy: Option<crate::ToolPolicy>,
    #[serde(default)]
    pool_routing: Option<crate::PoolRoutingPolicy>,
    #[serde(default)]
    basis_points_enabled: bool,
    max_retry_candidates: u8,
    default_service_tier: DefaultServiceTier,
    image_base_model: Option<String>,
    #[serde(default, rename = "cooldownAfterFailures")]
    _legacy_cooldown_after_failures: Option<serde::de::IgnoredAny>,
    #[serde(default, rename = "keepLastCandidateAvailable")]
    _legacy_keep_last_candidate_available: Option<serde::de::IgnoredAny>,
    #[serde(default, rename = "routingStrategy")]
    _legacy_routing_strategy: Option<serde::de::IgnoredAny>,
    #[serde(default, rename = "subscriptionPlanOrder")]
    _legacy_subscription_plan_order: Option<serde::de::IgnoredAny>,
}

impl<'de> Deserialize<'de> for PresetRoutingPolicy {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let reader = PresetRoutingPolicyReader::deserialize(deserializer)?;
        Ok(Self {
            tool_policy: reader.tool_policy,
            pool_routing: reader.pool_routing,
            basis_points_enabled: reader.basis_points_enabled,
            max_retry_candidates: reader.max_retry_candidates,
            default_service_tier: reader.default_service_tier,
            image_base_model: reader.image_base_model,
        })
    }
}
