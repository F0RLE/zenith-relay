use super::{BridgeState, InFlight};

use super::super::MAX_ACTIVE_RESPONSES;

impl BridgeState {
    pub(super) fn can_start_lane(&self, lane: Option<&str>) -> bool {
        self.pending_retries.is_empty()
            && self.pending_admission.is_none()
            && self.active_count() < MAX_ACTIVE_RESPONSES
            && !self.lane_busy(lane)
    }

    pub(super) fn active_count(&self) -> usize {
        usize::from(self.in_flight.is_some()) + self.other_requests.len()
    }

    pub(super) fn lane_busy(&self, lane: Option<&str>) -> bool {
        self.in_flight
            .as_ref()
            .is_some_and(|request| request.request.stream_id.as_deref() == lane)
            || self.other_requests.keys().any(|key| key.as_deref() == lane)
            || self
                .pending_retries
                .iter()
                .any(|(_, request)| request.stream_id.as_deref() == lane)
    }

    pub(super) fn park_current_request(&mut self) {
        if let Some(request) = self.in_flight.take() {
            let replaced = self
                .other_requests
                .insert(request.request.stream_id.clone(), request);
            debug_assert!(
                replaced.is_none(),
                "a lane must have exactly one active owner"
            );
        }
    }

    pub(super) fn focus_lane(&mut self, lane: Option<&str>) -> bool {
        if self
            .in_flight
            .as_ref()
            .is_some_and(|request| request.request.stream_id.as_deref() == lane)
        {
            return true;
        }
        let key = self
            .other_requests
            .keys()
            .find(|key| key.as_deref() == lane)
            .cloned();
        let Some(key) = key else {
            return false;
        };
        self.park_current_request();
        self.in_flight = self.other_requests.remove(&key);
        true
    }

    pub(super) fn focus_response(&mut self, response_id: &str) -> bool {
        let lane = self
            .in_flight
            .iter()
            .chain(self.other_requests.values())
            .find(|request| request.response_id.as_deref() == Some(response_id))
            .map(|request| request.request.stream_id.clone());
        lane.is_some_and(|lane| self.focus_lane(lane.as_deref()))
    }

    pub(super) fn focus_remaining_request(&mut self) -> bool {
        if self.in_flight.is_none() {
            self.in_flight = self.other_requests.pop_first().map(|(_, request)| request);
        }
        self.in_flight.is_some()
    }

    pub(super) fn active_requests(&self) -> impl Iterator<Item = &InFlight> {
        self.in_flight.iter().chain(self.other_requests.values())
    }
}
