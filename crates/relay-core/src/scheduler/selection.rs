mod availability;
mod lifecycle;
mod members;
mod ownership;
mod projection;
mod quota;
mod recovery;
mod registry;
mod reservations;
mod snapshot;

pub(crate) use reservations::ReservationId;
use reservations::Reservations;

use super::activity::{InFlightLane, SchedulerActivity};
use super::affinity::AffinityCache;
use super::candidate::{
    CandidateHealth, CandidateKind, CandidateQuotaState, CandidateScope, RuntimeCandidate,
};
use super::capacity::{CandidateQuota, QUOTA_STALE_AFTER_MS};
use super::cooldown::CooldownReason;
use super::rotation::{
    AttemptId as RotationAttemptId, AttemptObservation as RotationAttemptObservation,
    AuthState as RotationAuthState, QuotaState as RotationQuotaState,
    RateState as RotationRateState, RequestBudget as RotationRequestBudget,
    RequestId as RotationRequestId, RotationCandidate, RotationEngine, RotationLease, RotationMode,
    RotationOperation, RotationRequest, RotationSettlement,
    SettlementError as RotationSettlementError,
};
use crate::WireApi;
use std::collections::{BTreeMap, BTreeSet, HashSet};

pub use snapshot::{
    ActiveModelRuntime, CandidateRuntimeSnapshot, ModelRetryRuntime, RoutingDiagnostics,
    SelectionReason,
};

// Keep response ownership across busy personal pools without allowing an
// unbounded in-memory map. The durable store uses the same capacity policy.
const RESPONSE_AFFINITY_MAX_ENTRIES: usize = 16_384;
pub const RESPONSE_AFFINITY_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
const PROMPT_AFFINITY_MAX_ENTRIES: usize = 16_384;
pub const PROMPT_AFFINITY_TTL_MS: u64 = 60 * 60 * 1_000;
const MAX_OAUTH_IMAGE_IN_FLIGHT: u32 = 1;

pub struct SelectionRequest<'a> {
    pub model: &'a str,
    pub allowed_protocols: &'a [WireApi],
    pub scope: &'a CandidateScope,
    pub tried: &'a HashSet<String>,
    pub response_affinity_key: Option<&'a str>,
    pub prompt_affinity_key: Option<&'a str>,
    pub now_ms: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct CooldownRequest<'a> {
    pub(crate) scope: &'a str,
    pub(crate) retry_at_ms: u64,
    pub(crate) reason: CooldownReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    pub candidate_id: String,
    pub response_affinity_hit: bool,
    pub half_open_probe: bool,
    pub diagnostics: RoutingDiagnostics,
    pub(crate) rotation_request: RotationRequest,
}

#[derive(Clone, Debug)]
pub struct PoolScheduler {
    candidates: BTreeMap<String, RuntimeCandidate>,
    cooldown_reasons: BTreeMap<(String, String), CooldownReason>,
    response_affinity: AffinityCache,
    prompt_affinity: AffinityCache,
    activity: SchedulerActivity,
    member_activity: SchedulerActivity,
    pool_routing: Option<crate::PoolRoutingPolicy>,
    native_routes: BTreeSet<String>,
    reservations: Reservations,
    quota_stale_after_ms: u64,
    protected_candidate: Option<(String, u64)>,
    execution_fences: BTreeMap<String, (u64, u32)>,
    next_execution_fence_epoch: u64,
    capability_blocks: BTreeSet<(String, String)>,
    /// Only structural permission edits advance this fence. Priority and
    /// weight updates leave already reserved, still-permitted work intact.
    candidate_permission_revisions: BTreeMap<String, u64>,
    /// Removed candidates remain as tombstones while leases are active.
    retired_candidates: BTreeSet<String>,
    /// The sole admission engine; candidate records hold provider observations.
    rotation: RotationEngine,
    rotation_leases: BTreeMap<ReservationId, RotationLease>,
    /// A superseded runtime may settle existing work, but cannot start more.
    retired: bool,
}

impl Default for PoolScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl PoolScheduler {
    pub fn new() -> Self {
        Self {
            candidates: BTreeMap::new(),
            cooldown_reasons: BTreeMap::new(),
            response_affinity: AffinityCache::new(
                RESPONSE_AFFINITY_MAX_ENTRIES,
                RESPONSE_AFFINITY_TTL_MS,
            ),
            prompt_affinity: AffinityCache::new(
                PROMPT_AFFINITY_MAX_ENTRIES,
                PROMPT_AFFINITY_TTL_MS,
            ),
            activity: SchedulerActivity::default(),
            member_activity: SchedulerActivity::default(),
            pool_routing: None,
            native_routes: BTreeSet::new(),
            reservations: Reservations::default(),
            quota_stale_after_ms: QUOTA_STALE_AFTER_MS,
            protected_candidate: None,
            execution_fences: BTreeMap::new(),
            next_execution_fence_epoch: 0,
            capability_blocks: BTreeSet::new(),
            candidate_permission_revisions: BTreeMap::new(),
            retired_candidates: BTreeSet::new(),
            rotation: RotationEngine::default(),
            rotation_leases: BTreeMap::new(),
            retired: false,
        }
    }

    pub(crate) fn retire_for_replacement(&mut self) {
        self.retired = true;
    }

    pub(crate) fn is_retired(&self) -> bool {
        self.retired
    }

    pub fn select(&mut self, request: SelectionRequest<'_>) -> Option<Selection> {
        self.select_for_operation(request, InFlightLane::Text, RotationOperation::Text)
    }

    pub(crate) fn select_image(&mut self, request: SelectionRequest<'_>) -> Option<Selection> {
        self.select_for_operation(request, InFlightLane::Image, RotationOperation::Image)
    }

    pub(crate) fn select_compaction(&mut self, request: SelectionRequest<'_>) -> Option<Selection> {
        self.select_for_operation(request, InFlightLane::Text, RotationOperation::Compaction)
    }

    fn select_for_operation(
        &mut self,
        request: SelectionRequest<'_>,
        lane: InFlightLane,
        operation: RotationOperation,
    ) -> Option<Selection> {
        let mut rotation_request = self.prepare_rotation_request(&request, operation)?;
        if let Some(allowed) = &mut rotation_request.allowed_candidates {
            allowed.retain(|id| self.lane_allows(&self.candidates[id], lane));
        }
        let owner = rotation_request.owner.clone();
        let selected = self.rotation.select(&rotation_request, request.now_ms)?;
        let reason = if owner.is_none()
            && !selected.recovery
            && self.rotation.mode() == RotationMode::Automatic
            && rotation_request.preferred.as_deref() == Some(selected.candidate_id.as_str())
        {
            SelectionReason::PromptCacheAffinity
        } else {
            match selected.reason {
                super::rotation::RotationSelectionReason::HardOwner if owner.is_some() => {
                    SelectionReason::ResponseAffinity
                }
                super::rotation::RotationSelectionReason::LeastLoaded => {
                    SelectionReason::ParallelLoad
                }
                super::rotation::RotationSelectionReason::PrimaryFirst => {
                    SelectionReason::ManualPriority
                }
                super::rotation::RotationSelectionReason::WeightedRotation => {
                    SelectionReason::FairRotation
                }
                super::rotation::RotationSelectionReason::Recovery => {
                    SelectionReason::FallbackAttempt
                }
                super::rotation::RotationSelectionReason::HardOwner
                | super::rotation::RotationSelectionReason::OnlyEligible => {
                    SelectionReason::OnlyEligible
                }
            }
        };
        let diagnostics = self.diagnostics(
            &selected.candidate_id,
            reason,
            usize::try_from(selected.eligible_candidates).unwrap_or(usize::MAX),
            lane,
        )?;
        Some(Selection {
            candidate_id: selected.candidate_id,
            response_affinity_hit: owner.is_some(),
            half_open_probe: selected.recovery,
            diagnostics,
            rotation_request,
        })
    }
}

#[cfg(test)]
mod tests;
