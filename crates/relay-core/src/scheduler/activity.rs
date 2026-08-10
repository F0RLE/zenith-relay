use std::collections::BTreeMap;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum InFlightLane {
    Text,
    Image,
}

#[derive(Clone, Debug, Default)]
pub(super) struct SchedulerActivity {
    text_in_flight: BTreeMap<String, u32>,
    image_in_flight: BTreeMap<String, u32>,
    active_models: BTreeMap<String, BTreeMap<String, u32>>,
    text_dispatches: BTreeMap<String, u64>,
    image_dispatches: BTreeMap<String, u64>,
}

impl SchedulerActivity {
    pub(super) fn clear_dispatches(&mut self) {
        self.text_dispatches.clear();
        self.image_dispatches.clear();
    }

    pub(super) fn remove_candidate(&mut self, candidate_id: &str) {
        self.text_in_flight.remove(candidate_id);
        self.image_in_flight.remove(candidate_id);
        self.active_models.remove(candidate_id);
        self.text_dispatches.remove(candidate_id);
        self.image_dispatches.remove(candidate_id);
    }

    pub(super) fn reserve(&mut self, candidate_id: &str, model: &str, lane: InFlightLane) {
        let in_flight = self
            .in_flight_map_mut(lane)
            .entry(candidate_id.to_string())
            .or_default();
        *in_flight = in_flight.saturating_add(1);

        let dispatches = self
            .dispatch_map_mut(lane)
            .entry(candidate_id.to_string())
            .or_default();
        *dispatches = dispatches.saturating_add(1);

        let request_count = self
            .active_models
            .entry(candidate_id.to_string())
            .or_default()
            .entry(model.to_ascii_lowercase())
            .or_default();
        *request_count = request_count.saturating_add(1);
    }

    pub(super) fn release(
        &mut self,
        candidate_id: &str,
        model: Option<&str>,
        lane: InFlightLane,
    ) -> bool {
        {
            let in_flight_map = self.in_flight_map_mut(lane);
            let Some(in_flight) = in_flight_map.get_mut(candidate_id) else {
                return false;
            };
            if *in_flight <= 1 {
                in_flight_map.remove(candidate_id);
            } else {
                *in_flight -= 1;
            }
        }

        self.release_active_model(candidate_id, model);
        true
    }

    pub(super) fn in_flight_count(&self, candidate_id: &str, lane: InFlightLane) -> u32 {
        self.in_flight_map(lane)
            .get(candidate_id)
            .copied()
            .unwrap_or_default()
    }

    pub(super) fn active_request_count(&self, candidate_id: &str) -> u32 {
        self.active_models
            .get(candidate_id)
            .into_iter()
            .flat_map(|models| models.values())
            .fold(0_u32, |count, model_count| {
                count.saturating_add(*model_count)
            })
    }

    pub(super) fn active_models_for(&self, candidate_id: &str) -> Vec<(String, u32)> {
        self.active_models
            .get(candidate_id)
            .into_iter()
            .flat_map(|models| models.iter())
            .filter(|(model, count)| !model.is_empty() && **count > 0)
            .map(|(model, request_count)| (model.clone(), *request_count))
            .collect()
    }

    pub(super) fn dispatch_count(&self, candidate_id: &str, lane: InFlightLane) -> u64 {
        self.dispatch_map(lane)
            .get(candidate_id)
            .copied()
            .unwrap_or_default()
    }

    fn release_active_model(&mut self, candidate_id: &str, model: Option<&str>) {
        let model_key = model
            .map(str::to_ascii_lowercase)
            .or_else(|| self.active_models.get(candidate_id)?.keys().next().cloned());
        let Some(model_key) = model_key else {
            return;
        };
        let empty = {
            let Some(models) = self.active_models.get_mut(candidate_id) else {
                return;
            };
            let remove_model = models.get(&model_key).is_some_and(|count| *count <= 1);
            if remove_model {
                models.remove(&model_key);
            } else if let Some(request_count) = models.get_mut(&model_key) {
                *request_count -= 1;
            } else {
                return;
            }
            models.is_empty()
        };
        if empty {
            self.active_models.remove(candidate_id);
        }
    }

    fn in_flight_map(&self, lane: InFlightLane) -> &BTreeMap<String, u32> {
        match lane {
            InFlightLane::Text => &self.text_in_flight,
            InFlightLane::Image => &self.image_in_flight,
        }
    }

    fn in_flight_map_mut(&mut self, lane: InFlightLane) -> &mut BTreeMap<String, u32> {
        match lane {
            InFlightLane::Text => &mut self.text_in_flight,
            InFlightLane::Image => &mut self.image_in_flight,
        }
    }

    fn dispatch_map(&self, lane: InFlightLane) -> &BTreeMap<String, u64> {
        match lane {
            InFlightLane::Text => &self.text_dispatches,
            InFlightLane::Image => &self.image_dispatches,
        }
    }

    fn dispatch_map_mut(&mut self, lane: InFlightLane) -> &mut BTreeMap<String, u64> {
        match lane {
            InFlightLane::Text => &mut self.text_dispatches,
            InFlightLane::Image => &mut self.image_dispatches,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InFlightLane, SchedulerActivity};

    #[test]
    fn keeps_text_and_image_activity_separate() {
        let mut activity = SchedulerActivity::default();
        activity.reserve("candidate", "gpt-5", InFlightLane::Text);
        activity.reserve("candidate", "gpt-image-2", InFlightLane::Image);

        assert_eq!(activity.in_flight_count("candidate", InFlightLane::Text), 1);
        assert_eq!(
            activity.in_flight_count("candidate", InFlightLane::Image),
            1
        );

        assert!(activity.release("candidate", Some("gpt-image-2"), InFlightLane::Image));
        assert_eq!(activity.in_flight_count("candidate", InFlightLane::Text), 1);
        assert_eq!(
            activity.in_flight_count("candidate", InFlightLane::Image),
            0
        );
    }

    #[test]
    fn groups_active_requests_by_normalized_model() {
        let mut activity = SchedulerActivity::default();
        activity.reserve("candidate", "GPT-5", InFlightLane::Text);
        activity.reserve("candidate", "gpt-5", InFlightLane::Text);
        activity.reserve("candidate", "claude-opus-5", InFlightLane::Image);

        assert_eq!(activity.active_request_count("candidate"), 3);
        assert_eq!(
            activity.active_models_for("candidate"),
            vec![("claude-opus-5".to_string(), 1), ("gpt-5".to_string(), 2)],
        );

        assert!(activity.release("candidate", Some("gpt-5"), InFlightLane::Text));
        assert_eq!(
            activity.active_models_for("candidate"),
            vec![("claude-opus-5".to_string(), 1), ("gpt-5".to_string(), 1)],
        );
    }

    #[test]
    fn removes_all_activity_for_a_removed_candidate() {
        let mut activity = SchedulerActivity::default();
        activity.reserve("removed", "gpt-5", InFlightLane::Text);
        activity.reserve("removed", "gpt-image-2", InFlightLane::Image);
        activity.reserve("kept", "gpt-5", InFlightLane::Text);

        activity.remove_candidate("removed");

        assert_eq!(activity.active_request_count("removed"), 0);
        assert!(activity.active_models_for("removed").is_empty());
        assert_eq!(activity.dispatch_count("removed", InFlightLane::Text), 0);
        assert_eq!(activity.in_flight_count("kept", InFlightLane::Text), 1);
    }

    #[test]
    fn clears_rotation_accounting_without_releasing_active_requests() {
        let mut activity = SchedulerActivity::default();
        activity.reserve("candidate", "gpt-5", InFlightLane::Text);
        activity.reserve("candidate", "gpt-image-2", InFlightLane::Image);

        activity.clear_dispatches();

        assert_eq!(activity.dispatch_count("candidate", InFlightLane::Text), 0);
        assert_eq!(activity.dispatch_count("candidate", InFlightLane::Image), 0);
        assert_eq!(activity.active_request_count("candidate"), 2);
    }
}
