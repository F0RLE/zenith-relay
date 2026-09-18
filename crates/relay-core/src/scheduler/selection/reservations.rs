use super::*;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ReservationId(u64);

#[derive(Clone, Debug)]
struct Reservation {
    candidate_id: String,
    member_key: String,
    model: String,
    lane: InFlightLane,
    probe_scope: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct Reservations {
    next_id: u64,
    active: BTreeMap<ReservationId, Reservation>,
    probes: BTreeMap<(String, String), ReservationId>,
}

impl Reservations {
    pub(super) fn probe_available(&self, candidate_id: &str, model: &str) -> bool {
        !self
            .probes
            .contains_key(&(candidate_id.to_owned(), "*".into()))
            && !self
                .probes
                .contains_key(&(candidate_id.to_owned(), model.to_ascii_lowercase()))
    }

    pub(super) fn is_probing(&self, candidate_id: &str) -> bool {
        self.probes.keys().any(|(id, _)| id == candidate_id)
    }

    pub(super) fn invalidate_probe(&mut self, candidate_id: &str, scope: &str) {
        self.probes
            .retain(|(id, model), _| id != candidate_id || (scope != "*" && model != scope));
    }

    pub(super) fn remove_candidate(&mut self, candidate_id: &str) {
        self.active
            .retain(|_, request| request.candidate_id != candidate_id);
        self.invalidate_probe(candidate_id, "*");
    }

    fn reserve(&mut self, reservation: Reservation) -> Option<ReservationId> {
        self.next_id = self.next_id.checked_add(1)?;
        let id = ReservationId(self.next_id);
        if let Some(scope) = &reservation.probe_scope {
            self.probes
                .insert((reservation.candidate_id.clone(), scope.clone()), id);
        }
        self.active.insert(id, reservation);
        Some(id)
    }

    fn release(&mut self, id: ReservationId) -> Option<Reservation> {
        let reservation = self.active.remove(&id)?;
        if let Some(scope) = &reservation.probe_scope {
            let key = (reservation.candidate_id.clone(), scope.clone());
            // A newer cooldown can have replaced this probe while its request
            // was still finishing. Only the reservation that owns it may release it.
            if self.probes.get(&key) == Some(&id) {
                self.probes.remove(&key);
            }
        }
        Some(reservation)
    }
}

impl PoolScheduler {
    pub(crate) fn reserve_request(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
        image: bool,
    ) -> Option<ReservationId> {
        let lane = if image {
            InFlightLane::Image
        } else {
            InFlightLane::Text
        };
        let candidate = self.candidates.get(candidate_id)?;
        if !self.lane_allows(candidate, lane)
            || !self.reservations.probe_available(candidate_id, model)
        {
            return None;
        }
        let member_key = unified::member_key(candidate);
        let reservation = self.reservations.reserve(Reservation {
            candidate_id: candidate_id.to_owned(),
            member_key: member_key.clone(),
            model: model.to_owned(),
            lane,
            probe_scope: (!model.is_empty())
                .then(|| half_open_scope(candidate, model, now_ms))
                .flatten(),
        })?;
        self.member_activity.reserve(&member_key, model, lane);
        self.activity.reserve(candidate_id, model, lane);
        Some(reservation)
    }

    pub(crate) fn release_reservation(&mut self, id: ReservationId) -> bool {
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
        self.reserve_for(candidate_id, "", 0)
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
