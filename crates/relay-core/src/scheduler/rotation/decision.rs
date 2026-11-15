//! On-demand admission evidence for one fixed request and scheduler snapshot.

use super::{CandidateBlockReason, RotationMode};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationDecisionStageKind {
    Eligibility,
    PhysicalMember,
    RecoveryBudget,
    Recovery,
    ManualOrder,
    Health,
    SubscriptionExpiry,
    ParallelLoad,
    ResourceHeadroom,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationDecisionStage {
    pub kind: RotationDecisionStageKind,
    pub remaining: usize,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationDecisionAvailability {
    Ready,
    Busy,
    Waiting,
    Blocked,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RotationConfigurationBlock {
    RuntimeRetired,
    CandidateRetired,
    ExecutionFence,
    CapabilityBlocked,
    QuotaReserve,
    Disabled,
    Draining,
    SecretMissing,
    ProtocolUnsupported,
    ModelUnsupported,
    ModelRule,
    Scope,
    ScopeModelRule,
    PhysicalAlias,
    LaneCapacity,
    RequestPreparation,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationExpiryEvidence {
    pub expires_at_ms: u64,
    pub valid_until_ms: u64,
    /// Zero-based three-day band, calculated at the snapshot's observation time.
    pub band: u64,
    pub reset_credits: Option<u32>,
    /// Potential urgency from fresh evidence; effective_weight records whether
    /// this request's final comparison actually applies it.
    pub urgency_multiplier: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationCandidateDecision {
    pub candidate_id: String,
    /// Fallback preference after excluding earlier physical members, without
    /// advancing credits or pretending to dispatch future requests.
    pub fallback_rank: Option<usize>,
    pub availability: RotationDecisionAvailability,
    pub block_reason: Option<CandidateBlockReason>,
    pub configuration_block: Option<RotationConfigurationBlock>,
    pub retry_at_ms: Option<u64>,
    pub eliminated_at: Option<RotationDecisionStageKind>,
    pub priority: i32,
    pub configured_weight: u32,
    /// Actual weight in the first selection's final comparison, when admitted.
    pub effective_weight: Option<u32>,
    pub rotation_score: Option<i64>,
    pub in_flight: u32,
    pub effective_capacity: u32,
    pub quota_remaining_basis_points: Option<u64>,
    pub provider_credits_micro_units: Option<u64>,
    pub provider_credits_unlimited: bool,
    pub expiry: Option<RotationExpiryEvidence>,
    pub preferred: bool,
    pub warm_alternative: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationDecisionTrace {
    pub observed_at_ms: u64,
    pub mode: RotationMode,
    pub selected_candidate_id: Option<String>,
    pub owner_candidate_id: Option<String>,
    /// False when the bounded fallback walk stops before exhausting the pool.
    pub fallback_order_complete: bool,
    pub stages: Vec<RotationDecisionStage>,
    pub candidates: Vec<RotationCandidateDecision>,
}
