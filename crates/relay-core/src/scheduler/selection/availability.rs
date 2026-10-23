//! Read projections of the same admission state used by request selection.

use super::*;
use crate::scheduler::rotation::{CandidateAvailability, CandidateBlockReason};

impl PoolScheduler {
    pub(crate) fn all_capacity_reserved(&mut self) -> bool {
        // Candidate changes are projected lazily. An empty/stale rotation registry
        // must never turn a newly configured, free route into a false reject.
        self.sync_all_rotation_candidates();
        self.rotation.all_capacity_reserved()
    }

    /// Recheck admission-only permission and evidence for a reserved route.
    /// The rotation engine checks auth/quota/rate/circuit generations separately;
    /// this closes the gap for an intervening Auth fence, capability removal
    /// or a newly protected quota reserve before the actual wire dispatch.
    pub(crate) fn dispatch_visible(
        &self,
        candidate_id: &str,
        model: &str,
        protocols: &[WireApi],
        scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        self.candidates.get(candidate_id).is_some_and(|candidate| {
            self.rotation_visible(candidate, model, protocols, scope, now_ms)
        })
    }

    pub(crate) fn admission_ready_for(
        &mut self,
        selection_request: SelectionRequest<'_>,
        operation: RotationOperation,
    ) -> bool {
        let lane = if operation == RotationOperation::Image {
            InFlightLane::Image
        } else {
            InFlightLane::Text
        };
        self.select_for_operation(selection_request, lane, operation)
            .is_some()
    }

    pub(super) fn model_operation(model: &str) -> RotationOperation {
        if crate::runtime::is_image_model_id(model) {
            RotationOperation::Image
        } else {
            RotationOperation::Text
        }
    }

    pub(crate) fn capacity_blocked_for(
        &mut self,
        selection_request: SelectionRequest<'_>,
        operation: RotationOperation,
    ) -> bool {
        let Some(projected) = self.prepare_rotation_request(&selection_request, operation) else {
            return false;
        };
        self.candidates.values().any(|candidate| {
            match self.rotation.candidate_availability(
                &projected,
                &candidate.id,
                selection_request.now_ms,
            ) {
                CandidateAvailability::Busy(_) => true,
                CandidateAvailability::Ready { .. } => {
                    operation == RotationOperation::Image
                        && !self.lane_allows(candidate, InFlightLane::Image)
                }
                _ => false,
            }
        })
    }

    pub(crate) fn recovery_retry_at_for(
        &mut self,
        selection_request: SelectionRequest<'_>,
        operation: RotationOperation,
    ) -> Option<u64> {
        let projected = self.prepare_rotation_request(&selection_request, operation)?;
        // A timer may have elapsed while another request completed. In that
        // case the caller can immediately retry, using its original budget.
        if self
            .rotation
            .select(&projected, selection_request.now_ms)
            .is_some()
        {
            return Some(selection_request.now_ms);
        }
        self.rotation
            .next_wakeup(&projected, selection_request.now_ms)
    }

    pub fn earliest_retry_at(&mut self, selection_request: SelectionRequest<'_>) -> Option<u64> {
        let operation = Self::model_operation(selection_request.model);
        self.earliest_retry_at_for(selection_request, operation)
    }

    pub(crate) fn earliest_retry_at_for(
        &mut self,
        selection_request: SelectionRequest<'_>,
        operation: RotationOperation,
    ) -> Option<u64> {
        let projected = self.prepare_rotation_request(&selection_request, operation)?;
        self.rotation
            .next_wakeup(&projected, selection_request.now_ms)
    }

    #[cfg(test)]
    pub(crate) fn all_applicable_cooldown(
        &mut self,
        selection_request: SelectionRequest<'_>,
    ) -> Option<(u64, CooldownReason)> {
        let operation = Self::model_operation(selection_request.model);
        self.all_applicable_cooldown_for(selection_request, operation)
    }

    pub(crate) fn all_applicable_cooldown_for(
        &mut self,
        selection_request: SelectionRequest<'_>,
        operation: RotationOperation,
    ) -> Option<(u64, CooldownReason)> {
        let projected = self.prepare_rotation_request(&selection_request, operation)?;
        let mut deadline: Option<u64> = None;
        let mut reason = CooldownReason::RateLimit;
        for candidate in self.candidates.values() {
            match self.rotation.candidate_availability(
                &projected,
                &candidate.id,
                selection_request.now_ms,
            ) {
                CandidateAvailability::WaitUntil {
                    at_ms,
                    reason: block,
                } => {
                    deadline =
                        Some(deadline.map_or(at_ms, |known_deadline| known_deadline.min(at_ms)));
                    let next_reason = match block {
                        CandidateBlockReason::CircuitOpen => CooldownReason::Transient,
                        CandidateBlockReason::QuotaExhausted => CooldownReason::Mandatory,
                        _ => self.cooldown_reason_for(
                            candidate,
                            selection_request.model,
                            selection_request.now_ms,
                        ),
                    };
                    reason = Self::aggregate_cooldown_reason(reason, next_reason);
                }
                CandidateAvailability::Ready { .. } | CandidateAvailability::Busy(_) => {
                    return None
                }
                CandidateAvailability::Blocked(_) => {}
            }
        }
        deadline.map(|at| (at, reason))
    }

    pub(crate) fn is_eligible(
        &self,
        candidate: &RuntimeCandidate,
        model: &str,
        protocols: &[WireApi],
        scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        if !self.rotation_visible(candidate, model, protocols, scope, now_ms) {
            return false;
        }
        let mut projection = self.clone();
        projection.sync_all_rotation_candidates();
        let candidate_request = projection.rotation_request(
            None,
            Some(&candidate.id),
            model,
            Self::model_operation(model),
            BTreeSet::from([candidate.id.clone()]),
        );
        matches!(
            projection
                .rotation
                .candidate_availability(&candidate_request, &candidate.id, now_ms,),
            CandidateAvailability::Ready { .. }
        )
    }
}

impl PoolScheduler {
    pub(super) fn cooldown_reason_for(
        &self,
        candidate: &RuntimeCandidate,
        model: &str,
        now_ms: u64,
    ) -> CooldownReason {
        let mut reason = None;
        for (candidate_model, retry_at) in &candidate.cooldowns {
            if *retry_at <= now_ms
                || (candidate_model.as_str() != "*" && !candidate_model.eq_ignore_ascii_case(model))
            {
                continue;
            }
            let candidate_reason = self
                .cooldown_reasons
                .get(&(candidate.id.clone(), candidate_model.clone()))
                .copied()
                .unwrap_or(CooldownReason::Transient);
            reason = Some(Self::aggregate_cooldown_reason(
                reason.unwrap_or(CooldownReason::RateLimit),
                candidate_reason,
            ));
        }
        reason.unwrap_or(CooldownReason::Transient)
    }

    pub(super) fn aggregate_cooldown_reason(
        existing_reason: CooldownReason,
        incoming: CooldownReason,
    ) -> CooldownReason {
        match (existing_reason, incoming) {
            (CooldownReason::Mandatory, _) | (_, CooldownReason::Mandatory) => {
                CooldownReason::Mandatory
            }
            (CooldownReason::Transient, _) | (_, CooldownReason::Transient) => {
                CooldownReason::Transient
            }
            _ => CooldownReason::RateLimit,
        }
    }
}
