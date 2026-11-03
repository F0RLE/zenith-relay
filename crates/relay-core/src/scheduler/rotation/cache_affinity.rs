//! Selection-time evidence, not a claim about provider KV-cache residency.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheAffinitySelection {
    Preferred,
    WarmAlternative,
    Other,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheAffinityReason {
    Retained,
    NoBinding,
    RoutingPolicy,
    ResponseOwner,
    OwnerUnavailable,
    RecoveryProbe,
    ParallelLoad,
    QuotaHeadroom,
    ProviderCredits,
    Health,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheAffinityMember {
    /// Only fresh observations; missing values are not zero headroom.
    pub quota_remaining_basis_points: Option<u64>,
    pub provider_credits_micro_units: Option<u64>,
    pub provider_credits_unlimited: bool,
    pub in_flight: u32,
    pub effective_capacity: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CacheAffinityDiagnostics {
    pub selection: CacheAffinitySelection,
    pub reason: CacheAffinityReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_block_reason: Option<super::CandidateBlockReason>,
    pub preferred: Option<CacheAffinityMember>,
    pub selected: CacheAffinityMember,
    /// Resource baselines are taken after the actual load filter, before health.
    pub best_quota_remaining_basis_points: Option<u64>,
    pub best_provider_credits_micro_units: Option<u64>,
    pub best_provider_credits_unlimited: bool,
    pub quota_switch_margin_basis_points: u64,
    pub provider_credit_switch_margin_micro_units: u64,
    pub load_allowance_requests: u32,
}
