//! Dispatch and settlement bridge to the sole rotation lease owner.

use super::*;

impl PoolScheduler {
    pub(crate) fn begin_rotation_dispatch(
        &mut self,
        reservation_id: ReservationId,
        budget: &mut RotationRequestBudget,
    ) -> Result<RotationAttemptId, crate::scheduler::rotation::DispatchStartError> {
        if self.retired {
            return Err(crate::scheduler::rotation::DispatchStartError::CandidateChanged);
        }
        self.sync_all_rotation_candidates();
        let lease = self
            .rotation_leases
            .get(&reservation_id)
            .ok_or(crate::scheduler::rotation::DispatchStartError::UnknownLease)?;
        let attempt = self
            .rotation
            .begin_transport_dispatch(lease.lease_id, budget)?;
        self.record_reservation_dispatch(reservation_id);
        Ok(attempt)
    }

    pub(crate) fn circuit_state_for(&self, candidate_id: &str, model: &str) -> (u32, Option<u64>) {
        let states = [
            RotationOperation::Text,
            RotationOperation::Image,
            RotationOperation::Compaction,
        ]
        .map(|operation| {
            self.rotation
                .circuit(candidate_id, &Self::rotation_route_key(model, operation))
        });
        (
            states
                .iter()
                .map(|s| s.failure_streak)
                .max()
                .unwrap_or_default(),
            states.iter().filter_map(|s| s.not_before_ms).max(),
        )
    }

    pub(crate) fn settle_rotation(
        &mut self,
        reservation_id: ReservationId,
        observation: RotationAttemptObservation,
        budget: &RotationRequestBudget,
        now_ms: u64,
    ) -> Result<RotationSettlement, RotationSettlementError> {
        let lease = self
            .rotation_leases
            .get(&reservation_id)
            .cloned()
            .ok_or(RotationSettlementError::UnknownLease)?;
        let settlement = self
            .rotation
            .settle(lease.lease_id, observation, budget, now_ms)?;
        self.rotation_leases.remove(&reservation_id);
        Ok(settlement)
    }

    pub(crate) fn cancel_rotation(
        &mut self,
        reservation_id: ReservationId,
        budget: &RotationRequestBudget,
        now_ms: u64,
    ) -> Result<RotationSettlement, RotationSettlementError> {
        let lease = self
            .rotation_leases
            .get(&reservation_id)
            .cloned()
            .ok_or(RotationSettlementError::UnknownLease)?;
        let settlement = self.rotation.cancel(lease.lease_id, budget, now_ms)?;
        self.rotation_leases.remove(&reservation_id);
        Ok(settlement)
    }

    pub(crate) fn release_rotation_unstarted(&mut self, reservation_id: ReservationId) -> bool {
        let Some(lease) = self.rotation_leases.get(&reservation_id).cloned() else {
            return false;
        };
        let released = self.rotation.release_unstarted(lease.lease_id);
        if released {
            self.rotation_leases.remove(&reservation_id);
        }
        released
    }
}
