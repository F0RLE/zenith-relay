mod affinity;
mod candidate;
mod capacity;
mod cooldown;
mod selection;

pub use affinity::AffinityCache;
pub use candidate::{CandidateHealth, CandidateKind, CandidateScope, RuntimeCandidate};
pub use capacity::CandidateQuota;
pub use selection::{
    CandidateRuntimeSnapshot, PoolScheduler, RoutingDiagnostics, RoutingStrategy, Selection,
    SelectionReason, SelectionRequest, RESPONSE_AFFINITY_TTL_MS,
};
