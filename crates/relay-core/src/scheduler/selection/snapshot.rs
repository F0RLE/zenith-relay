use super::super::candidate::CandidateKind;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveModelRuntime {
    pub model: String,
    pub request_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRetryRuntime {
    pub model: String,
    pub retry_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateRuntimeSnapshot {
    pub candidate_id: String,
    pub kind: CandidateKind,
    pub available: bool,
    pub in_flight: u32,
    #[serde(default)]
    pub active_request_count: u32,
    #[serde(default)]
    pub active_models: Vec<ActiveModelRuntime>,
    /// Model-scoped cooldowns that are still active. The aggregate retry
    /// deadline remains useful for global cooldowns.
    #[serde(default)]
    pub model_retries: Vec<ModelRetryRuntime>,
    pub last_used_at_ms: Option<u64>,
    pub next_retry_at_ms: Option<u64>,
    pub half_open: bool,
    pub dispatches: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReason {
    ResponseAffinity,
    PromptCacheAffinity,
    OnlyEligible,
    RoutingTier,
    SourceRole,
    ParallelLoad,
    SourceLoad,
    PoolPolicy,
    QuotaHeadroom,
    SubscriptionExpiry,
    SubscriptionPlan,
    ManualPriority,
    FairRotation,
    FallbackAttempt,
    StableTieBreak,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDiagnostics {
    pub reason: SelectionReason,
    pub eligible_candidates: u32,
    pub quota_remaining_basis_points: Option<u64>,
    pub in_flight_before: u32,
    pub dispatches_before: u64,
    /// Safe endpoint classification populated once the executor has resolved
    /// the actual upstream route.  It intentionally contains no host, query,
    /// credential, or provider response data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_kind: Option<String>,
}
