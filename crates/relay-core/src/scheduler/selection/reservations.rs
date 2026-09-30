use super::*;
use crate::scheduler::rotation::RotationOperation;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ReservationId(u64);

#[derive(Clone, Debug)]
struct Reservation {
    candidate_id: String,
    member_key: String,
    model: String,
    lane: InFlightLane,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Reservations {
    next_id: u64,
    active: BTreeMap<ReservationId, Reservation>,
}

impl Reservations {
    pub(super) fn remove_candidate(&mut self, candidate_id: &str) {
        self.active
            .retain(|_, request| request.candidate_id != candidate_id);
    }

    fn reserve(&mut self, reservation: Reservation) -> Option<ReservationId> {
        self.next_id = self.next_id.checked_add(1)?;
        let id = ReservationId(self.next_id);
        self.active.insert(id, reservation);
        Some(id)
    }

    fn release(&mut self, id: ReservationId) -> Option<Reservation> {
        self.active.remove(&id)
    }
}

impl PoolScheduler {
    #[cfg(test)]
    pub(crate) fn reserve_request(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
        image: bool,
    ) -> Option<ReservationId> {
        let operation = if image {
            RotationOperation::Image
        } else {
            RotationOperation::Text
        };
        self.reserve_request_with_operation(
            candidate_id,
            model,
            now_ms,
            image,
            operation,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn reserve_request_with_operation(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
        image: bool,
        operation: RotationOperation,
        request_id: Option<RotationRequestId>,
        rotation_request: Option<&RotationRequest>,
    ) -> Option<ReservationId> {
        if self.retired {
            return None;
        }
        let lane = if image {
            InFlightLane::Image
        } else {
            InFlightLane::Text
        };
        let candidate = self.candidates.get(candidate_id)?.clone();
        if !self.lane_allows(&candidate, lane) {
            return None;
        }
        self.sync_all_rotation_candidates();
        let mut request = rotation_request.cloned().unwrap_or_else(|| {
            self.rotation_request(
                request_id,
                Some(candidate_id),
                model,
                operation,
                BTreeSet::from([candidate_id.to_owned()]),
            )
        });
        if let Some(request_id) = request_id {
            request.request_id = request_id;
        }
        let rotation_lease = self.rotation.reserve(&request, now_ms).ok()?;
        if rotation_lease.candidate_id != candidate_id {
            self.rotation.release_unstarted(rotation_lease.lease_id);
            return None;
        }
        let member_key = members::member_key(&candidate);
        let Some(reservation) = self.reservations.reserve(Reservation {
            candidate_id: candidate_id.to_owned(),
            member_key: member_key.clone(),
            model: model.to_owned(),
            lane,
        }) else {
            let _ = self.rotation.release_unstarted(rotation_lease.lease_id);
            return None;
        };
        self.member_activity.reserve(&member_key, model, lane);
        self.activity.reserve(candidate_id, model, lane);
        self.rotation_leases.insert(reservation, rotation_lease);
        Some(reservation)
    }

    pub(super) fn record_reservation_dispatch(&mut self, id: ReservationId) {
        if let Some(reservation) = self.reservations.active.get(&id) {
            self.activity
                .record_dispatch(&reservation.candidate_id, reservation.lane);
            self.member_activity
                .record_dispatch(&reservation.member_key, reservation.lane);
            if let Some(candidate) = self.candidates.get_mut(&reservation.candidate_id) {
                candidate.last_used_at = Some(crate::unix_time_ms());
            }
        }
    }

    pub(crate) fn release_reservation(&mut self, id: ReservationId) -> bool {
        if self.rotation_leases.contains_key(&id) && !self.release_rotation_unstarted(id) {
            return false;
        }
        let Some(reservation) = self.reservations.release(id) else {
            return false;
        };
        self.activity.release(
            &reservation.candidate_id,
            Some(&reservation.model),
            reservation.lane,
        );
        self.member_activity.release(
            &reservation.member_key,
            Some(&reservation.model),
            reservation.lane,
        );
        self.finalize_retired_if_idle(&reservation.candidate_id);
        true
    }
}

#[cfg(test)]
impl PoolScheduler {
    pub(crate) fn reserve(&mut self, candidate_id: &str) -> bool {
        let Some(model) = self
            .candidates
            .get(candidate_id)
            .and_then(|c| c.models.iter().next())
            .cloned()
        else {
            return false;
        };
        self.reserve_for(candidate_id, &model, 0)
    }

    pub(crate) fn reserve_for(&mut self, candidate_id: &str, model: &str, now_ms: u64) -> bool {
        self.reserve_request(candidate_id, model, now_ms, false)
            .is_some()
    }

    pub(crate) fn reserve_image_for(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> bool {
        self.reserve_request(candidate_id, model, now_ms, true)
            .is_some()
    }

    pub(crate) fn release(&mut self, candidate_id: &str) -> bool {
        self.release_for(candidate_id, None)
    }

    pub(crate) fn release_for(&mut self, candidate_id: &str, model: Option<&str>) -> bool {
        self.release_test_reservation(candidate_id, model, InFlightLane::Text)
    }

    pub(crate) fn release_image_for(&mut self, candidate_id: &str, model: Option<&str>) -> bool {
        self.release_test_reservation(candidate_id, model, InFlightLane::Image)
    }

    fn release_test_reservation(
        &mut self,
        candidate_id: &str,
        model: Option<&str>,
        lane: InFlightLane,
    ) -> bool {
        let id = self.reservations.active.iter().find_map(|(id, request)| {
            (request.candidate_id == candidate_id
                && request.lane == lane
                && model.is_none_or(|model| request.model.eq_ignore_ascii_case(model)))
            .then_some(*id)
        });
        id.is_some_and(|id| self.release_reservation(id))
    }
}

impl PoolScheduler {
    pub(super) fn lane_allows(&self, candidate: &RuntimeCandidate, lane: InFlightLane) -> bool {
        if !self.member_capacity_allows(candidate) {
            return false;
        }
        if candidate.kind != CandidateKind::OAuthAccount || lane == InFlightLane::Text {
            return true;
        }
        self.in_flight_count(&candidate.id, InFlightLane::Image) < MAX_OAUTH_IMAGE_IN_FLIGHT
    }

    pub(super) fn in_flight_count(&self, candidate_id: &str, lane: InFlightLane) -> u32 {
        self.activity.in_flight_count(candidate_id, lane)
    }

    pub(super) fn active_request_count(&self, candidate_id: &str) -> u32 {
        self.activity.active_request_count(candidate_id)
    }

    pub(super) fn active_models_for(&self, candidate_id: &str) -> Vec<ActiveModelRuntime> {
        self.activity
            .active_models_for(candidate_id)
            .into_iter()
            .map(|(model, request_count)| ActiveModelRuntime {
                model,
                request_count,
            })
            .collect()
    }

    pub(crate) fn runtime_activity_for(
        &self,
        candidate_id: &str,
    ) -> (u32, u32, Vec<ActiveModelRuntime>) {
        (
            self.in_flight_count(candidate_id, InFlightLane::Text),
            self.active_request_count(candidate_id),
            self.active_models_for(candidate_id),
        )
    }

    pub(super) fn dispatch_count(&self, candidate_id: &str, lane: InFlightLane) -> u64 {
        self.activity.dispatch_count(candidate_id, lane)
    }
}
