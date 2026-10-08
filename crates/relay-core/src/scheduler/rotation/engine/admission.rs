//! Eligibility, normalized load, recovery arbitration and weighted selection.

use super::*;

impl RotationEngine {
    pub fn candidate_availability(
        &self,
        request: &RotationRequest,
        candidate_id: &str,
        now_ms: u64,
    ) -> CandidateAvailability {
        if request.tried.contains(candidate_id) {
            return CandidateAvailability::Blocked(CandidateBlockReason::AlreadyTried);
        }
        if request
            .allowed_candidates
            .as_ref()
            .is_some_and(|allowed| !allowed.contains(candidate_id))
        {
            return CandidateAvailability::Blocked(CandidateBlockReason::NotAllowed);
        }
        if request
            .owner
            .as_deref()
            .is_some_and(|owner| owner != candidate_id)
        {
            return CandidateAvailability::Blocked(CandidateBlockReason::OwnerMismatch);
        }
        let Some(runtime) = self.candidates.get(candidate_id) else {
            return CandidateAvailability::Blocked(CandidateBlockReason::Disabled);
        };
        runtime
            .candidate
            .supports(request)
            .map_or_else(CandidateAvailability::Blocked, |_| {
                self.mutable_availability(runtime, &request.route_key, now_ms)
            })
    }

    /// Returns the earliest provider/local deadline that can make this
    /// request eligible.  Capacity has no timer and therefore returns `None`;
    /// callers should wake it from the lease/configuration notification.
    pub fn next_wakeup(&self, request: &RotationRequest, now_ms: u64) -> Option<u64> {
        self.candidates
            .keys()
            .filter_map(|candidate_id| {
                match self.candidate_availability(request, candidate_id, now_ms) {
                    CandidateAvailability::WaitUntil { at_ms, .. } => Some(at_ms),
                    _ => None,
                }
            })
            .min()
    }

    pub fn select(&self, request: &RotationRequest, now_ms: u64) -> Option<RotationSelection> {
        let (ready_candidates, _) = self.selection_candidates(request, now_ms);
        let least_loaded_candidates = self.least_loaded_group(&ready_candidates);
        let selection_group = self.selection_group(&ready_candidates, request, now_ms);
        let selected_candidate = self.choose_for_request(request, &selection_group)?;
        let selected_quota = self.fresh_quota_remaining(&selected_candidate.id, now_ms);
        let quota_influenced = request.owner.is_none()
            && !selected_candidate.recovery
            && self.mode == RotationMode::Automatic
            && selected_quota.is_some()
            && least_loaded_candidates.iter().any(|candidate| {
                self.fresh_quota_remaining(&candidate.id, now_ms) != selected_quota
            });
        let selected_provider_credits = self.fresh_provider_credits(&selected_candidate.id, now_ms);
        let provider_credits_influenced = request.owner.is_none()
            && !selected_candidate.recovery
            && self.mode == RotationMode::Automatic
            && least_loaded_candidates
                .iter()
                .all(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms).is_none())
            && selected_provider_credits.is_some()
            && least_loaded_candidates.iter().any(|candidate| {
                self.fresh_provider_credits(&candidate.id, now_ms) != selected_provider_credits
            });
        let load_influenced = least_loaded_candidates.len() < ready_candidates.len();
        let reason = if request.owner.is_some() {
            RotationSelectionReason::HardOwner
        } else if selected_candidate.recovery {
            RotationSelectionReason::Recovery
        } else if ready_candidates.len() == 1 {
            RotationSelectionReason::OnlyEligible
        } else if matches!(self.mode, RotationMode::InOrder | RotationMode::Manual) {
            RotationSelectionReason::ManualPriority
        } else if load_influenced && self.mode == RotationMode::Automatic {
            RotationSelectionReason::LeastLoaded
        } else if quota_influenced {
            RotationSelectionReason::QuotaHeadroom
        } else if provider_credits_influenced {
            RotationSelectionReason::ProviderCredits
        } else {
            RotationSelectionReason::WeightedRotation
        };
        Some(RotationSelection {
            candidate_id: selected_candidate.id.clone(),
            priority: selected_candidate.priority,
            recovery: selected_candidate.recovery,
            eligible_candidates: u32::try_from(ready_candidates.len()).unwrap_or(u32::MAX),
            reason,
        })
    }

    pub(super) fn selection_candidates(
        &self,
        request: &RotationRequest,
        now_ms: u64,
    ) -> (Vec<ReadyCandidate>, bool) {
        let ready_candidates = self.ready_candidates(request, now_ms);
        let ordinary_alternatives = ready_candidates.iter().any(|candidate| !candidate.recovery);
        let recovery_allowed = self.recovery_in_flight < self.recovery_policy.max_in_flight
            && (!ordinary_alternatives || self.recovery_credits > 0);
        let eligible_candidates = ready_candidates
            .into_iter()
            .filter(|candidate| !candidate.recovery || recovery_allowed)
            .collect::<Vec<_>>();
        (eligible_candidates, ordinary_alternatives)
    }

    pub(super) fn choose_for_request(
        &self,
        request: &RotationRequest,
        candidate_group: &[ReadyCandidate],
    ) -> Option<ReadyCandidate> {
        if self.mode == RotationMode::Automatic && request.owner.is_none() {
            if let Some(preferred) = candidate_group.iter().find(|candidate| {
                !candidate.recovery && request.preferred.as_deref() == Some(&candidate.id)
            }) {
                return Some(preferred.clone());
            }
        }
        if self.mode == RotationMode::Manual {
            return self.choose_manual(&request.route_key, candidate_group);
        }
        self.choose_weighted(&request.route_key, candidate_group)
    }

    fn choose_manual(
        &self,
        route_key: &str,
        candidates: &[ReadyCandidate],
    ) -> Option<ReadyCandidate> {
        let mut ordered_candidates = candidates.iter().collect::<Vec<_>>();
        ordered_candidates.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.id.cmp(&right.id))
        });
        if ordered_candidates.is_empty() {
            return None;
        }

        let previous_cursor = self.manual_cursor.get(route_key).map(|cursor| {
            let priority = self
                .candidates
                .values()
                .find(|runtime| runtime.candidate.capacity_key == cursor.capacity_key)
                .map_or(cursor.priority, |runtime| runtime.candidate.priority);
            (priority, cursor.candidate_id.as_str())
        });
        let next_index = previous_cursor
            .and_then(|(priority, candidate_id)| {
                ordered_candidates.iter().position(|candidate| {
                    candidate.priority < priority
                        || (candidate.priority == priority && candidate.id.as_str() > candidate_id)
                })
            })
            .unwrap_or(0);
        ordered_candidates
            .get(next_index)
            .map(|candidate| (*candidate).clone())
    }

    pub(crate) fn fresh_quota_remaining(&self, candidate_id: &str, now_ms: u64) -> Option<u64> {
        let runtime = self.candidates.get(candidate_id)?;
        if runtime.candidate.quota != QuotaState::Available {
            return None;
        }
        let remaining = runtime
            .candidate
            .quota_remaining_basis_points
            .filter(|remaining| *remaining > 0)?;
        if runtime
            .candidate
            .quota_observed_at_ms
            .is_some_and(|observed_at_ms| {
                now_ms.saturating_sub(observed_at_ms) > self.quota_stale_after_ms
            })
        {
            return None;
        }
        Some(remaining)
    }

    pub(crate) fn best_other_ordinary_fresh_quota(
        &self,
        request: &RotationRequest,
        owner_id: &str,
        now_ms: u64,
    ) -> Option<u64> {
        self.ready_candidates(request, now_ms)
            .into_iter()
            .filter(|candidate| !candidate.recovery && candidate.id != owner_id)
            .filter_map(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms))
            .max()
    }

    pub(crate) fn fresh_provider_credits(&self, candidate_id: &str, now_ms: u64) -> Option<u64> {
        let candidate = &self.candidates.get(candidate_id)?.candidate;
        let observed_at_ms = candidate.provider_credits_observed_at_ms?;
        if now_ms.saturating_sub(observed_at_ms) > self.quota_stale_after_ms {
            return None;
        }
        if candidate.provider_credits_unlimited {
            return Some(u64::MAX);
        }
        candidate
            .provider_credits_micro_units
            .filter(|credits| *credits > 0)
    }

    pub(crate) fn best_other_ordinary_fresh_provider_credits(
        &self,
        request: &RotationRequest,
        owner_id: &str,
        now_ms: u64,
    ) -> Option<u64> {
        self.ready_candidates(request, now_ms)
            .into_iter()
            .filter(|candidate| !candidate.recovery && candidate.id != owner_id)
            .filter_map(|candidate| self.fresh_provider_credits(&candidate.id, now_ms))
            .max()
    }

    fn least_loaded_group(&self, ready_candidates: &[ReadyCandidate]) -> Vec<ReadyCandidate> {
        let compare_load = |left: &ReadyCandidate, right: &ReadyCandidate| {
            (u64::from(left.in_flight) * u64::from(right.effective_capacity))
                .cmp(&(u64::from(right.in_flight) * u64::from(left.effective_capacity)))
        };
        let Some(best) = ready_candidates
            .iter()
            .min_by(|left, right| compare_load(left, right))
        else {
            return Vec::new();
        };
        ready_candidates
            .iter()
            .filter(|candidate| compare_load(candidate, best).is_eq())
            .cloned()
            .collect()
    }

    pub(super) fn selection_group(
        &self,
        ready: &[ReadyCandidate],
        request: &RotationRequest,
        now_ms: u64,
    ) -> Vec<ReadyCandidate> {
        // Exploration is arbitrated across the runtime, not separately for
        // each priority/model. A lower-priority due source must not starve.
        let recovery = ready
            .iter()
            .filter(|candidate| candidate.recovery)
            .min_by_key(|candidate| (candidate.due_at_ms, &candidate.id));
        if let Some(candidate) = recovery {
            return vec![candidate.clone()];
        }
        match self.mode {
            RotationMode::InOrder => ready
                .iter()
                .min_by(|left, right| {
                    right
                        .priority
                        .cmp(&left.priority)
                        .then_with(|| left.id.cmp(&right.id))
                })
                .cloned()
                .into_iter()
                .collect(),
            RotationMode::Manual => ready.to_vec(),
            RotationMode::RoundRobin => ready.to_vec(),
            RotationMode::Automatic => {
                // Spread independent concurrent work across the least-loaded
                // physical members first. Quota and credit balances decide
                // among members at the same normalized load.
                let least_loaded_candidates = self.least_loaded_group(ready);
                let best_quota = least_loaded_candidates
                    .iter()
                    .filter_map(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms))
                    .max();
                if let Some(best_quota) = best_quota {
                    return least_loaded_candidates
                        .iter()
                        .filter(|candidate| {
                            self.fresh_quota_remaining(&candidate.id, now_ms) == Some(best_quota)
                        })
                        .cloned()
                        .collect();
                }

                let best_credits = least_loaded_candidates
                    .iter()
                    .filter_map(|candidate| self.fresh_provider_credits(&candidate.id, now_ms))
                    .max();
                if let Some(best_credits) = best_credits {
                    if let Some(preferred) = least_loaded_candidates.iter().find(|candidate| {
                        request.preferred.as_deref() == Some(candidate.id.as_str())
                    }) {
                        if self
                            .fresh_provider_credits(&preferred.id, now_ms)
                            .is_some_and(|preferred_credits| {
                                best_credits.saturating_sub(preferred_credits)
                                    < PROVIDER_CREDIT_SWITCH_MARGIN_MICRO_UNITS
                            })
                        {
                            return vec![preferred.clone()];
                        }
                    }
                    return least_loaded_candidates
                        .iter()
                        .filter(|candidate| {
                            self.fresh_provider_credits(&candidate.id, now_ms) == Some(best_credits)
                        })
                        .cloned()
                        .collect();
                }
                least_loaded_candidates
            }
        }
    }

    pub(super) fn mutable_availability(
        &self,
        runtime: &CandidateRuntime,
        route_key: &str,
        now_ms: u64,
    ) -> CandidateAvailability {
        match runtime.candidate.auth {
            AuthState::Ready => {}
            AuthState::NeedsRefresh => {
                return CandidateAvailability::Blocked(CandidateBlockReason::AuthRequired)
            }
            AuthState::Blocked => {
                return CandidateAvailability::Blocked(CandidateBlockReason::AuthBlocked)
            }
        }
        let mut earliest_wait = None;
        match runtime.candidate.quota {
            QuotaState::Exhausted {
                reset_at_ms: Some(reset_at_ms),
            } if reset_at_ms > now_ms => {
                earliest_wait = Some((reset_at_ms, CandidateBlockReason::QuotaExhausted));
            }
            QuotaState::Exhausted { .. } => {
                return CandidateAvailability::Blocked(CandidateBlockReason::QuotaExhausted)
            }
            QuotaState::Unknown | QuotaState::Available | QuotaState::Stale => {}
        }
        let model_route_rate = runtime
            .candidate
            .route_rates
            .get(route_key)
            .copied()
            .unwrap_or(runtime.candidate.rate);
        for rate in [runtime.candidate.rate, model_route_rate] {
            if let RateState::Limited { not_before_ms } = rate {
                if not_before_ms > now_ms && earliest_wait.is_none_or(|(at, _)| not_before_ms > at)
                {
                    earliest_wait = Some((not_before_ms, CandidateBlockReason::RateLimited));
                }
            }
        }
        let circuit_state = self
            .circuits
            .get(&(runtime.candidate.id.clone(), route_key.to_owned()))
            .cloned()
            .unwrap_or_default();
        if let Some(not_before_ms) = circuit_state.not_before_ms.filter(|at| *at > now_ms) {
            if earliest_wait
                .is_none_or(|(previous_deadline_ms, _)| not_before_ms > previous_deadline_ms)
            {
                earliest_wait = Some((not_before_ms, CandidateBlockReason::CircuitOpen));
            }
        }
        if let Some((at_ms, reason)) = earliest_wait {
            return CandidateAvailability::WaitUntil { at_ms, reason };
        }
        if self.leases.len() >= self.max_in_flight as usize {
            return CandidateAvailability::Busy(CandidateBlockReason::CapacityBusy);
        }
        let capacity_limit = self
            .capacity_limits
            .get(&runtime.candidate.capacity_key)
            .copied()
            .unwrap_or_default();
        if capacity_limit > 0
            && self.capacity_in_flight(&runtime.candidate.capacity_key) >= capacity_limit
        {
            return CandidateAvailability::Busy(CandidateBlockReason::CapacityBusy);
        }
        match circuit_state.state {
            CircuitState::Closed => {}
            CircuitState::Degraded | CircuitState::Open => {
                if circuit_state.half_open_lease.is_some() {
                    return CandidateAvailability::Busy(CandidateBlockReason::RecoveryBusy);
                }
                if runtime.in_flight > 0
                    && runtime.candidate.max_concurrency > 0
                    && runtime.in_flight >= runtime.candidate.max_concurrency
                {
                    return CandidateAvailability::Busy(CandidateBlockReason::CapacityBusy);
                }
                return CandidateAvailability::Ready { recovery: true };
            }
            CircuitState::HalfOpen => {
                return CandidateAvailability::Busy(CandidateBlockReason::RecoveryBusy)
            }
        }
        if runtime.candidate.max_concurrency > 0
            && runtime.in_flight >= runtime.candidate.max_concurrency
        {
            return CandidateAvailability::Busy(CandidateBlockReason::CapacityBusy);
        }
        CandidateAvailability::Ready { recovery: false }
    }

    pub(super) fn ready_candidates(
        &self,
        request: &RotationRequest,
        now_ms: u64,
    ) -> Vec<ReadyCandidate> {
        self.candidates
            .values()
            .filter_map(|runtime| {
                if !matches!(
                    self.candidate_availability(request, &runtime.candidate.id, now_ms),
                    CandidateAvailability::Ready { .. }
                ) {
                    return None;
                }
                let recovery = matches!(
                    self.candidate_availability(request, &runtime.candidate.id, now_ms),
                    CandidateAvailability::Ready { recovery: true }
                );
                let due_at_ms = self
                    .circuits
                    .get(&(runtime.candidate.id.clone(), request.route_key.clone()))
                    .and_then(|circuit| circuit.not_before_ms);
                Some(ReadyCandidate {
                    id: runtime.candidate.id.clone(),
                    capacity_key: runtime.candidate.capacity_key.clone(),
                    in_flight: self.capacity_in_flight(&runtime.candidate.capacity_key),
                    effective_capacity: self
                        .capacity_limits
                        .get(&runtime.candidate.capacity_key)
                        .copied()
                        .filter(|limit| *limit > 0)
                        .unwrap_or(self.max_in_flight)
                        .min(self.max_in_flight),
                    priority: runtime.candidate.priority,
                    weight: if self.mode == RotationMode::Manual {
                        1
                    } else {
                        runtime.candidate.weight
                    },
                    recovery,
                    due_at_ms,
                })
            })
            // Aliases of a verified physical member carry one vote/weight.
            // Selection is stable among its compatible routes.
            .fold(
                BTreeMap::<String, ReadyCandidate>::new(),
                |mut physical_members, candidate| {
                    physical_members
                        .entry(candidate.capacity_key.clone())
                        .or_insert(candidate);
                    physical_members
                },
            )
            .into_values()
            .collect()
    }

    pub(super) fn choose_weighted(
        &self,
        route_key: &str,
        candidates: &[ReadyCandidate],
    ) -> Option<ReadyCandidate> {
        if candidates.iter().all(|candidate| candidate.recovery) {
            return candidates
                .iter()
                .min_by(|left, right| {
                    left.due_at_ms
                        .unwrap_or_default()
                        .cmp(&right.due_at_ms.unwrap_or_default())
                        .then_with(|| right.id.cmp(&left.id))
                })
                .cloned();
        }
        candidates
            .iter()
            .max_by(|left, right| {
                let left_rotation_credit = self
                    .rotation_credit
                    .get(&(route_key.to_owned(), left.capacity_key.clone()))
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(i64::from(left.weight));
                let right_rotation_credit = self
                    .rotation_credit
                    .get(&(route_key.to_owned(), right.capacity_key.clone()))
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(i64::from(right.weight));
                left_rotation_credit
                    .cmp(&right_rotation_credit)
                    // `max_by` wins the right-hand value on equality. Reverse the
                    // id order so the lexicographically smallest id is stable.
                    .then_with(|| right.id.cmp(&left.id))
            })
            .cloned()
    }

    pub(super) fn advance_rotation_state(
        &mut self,
        route_key: &str,
        candidates: &[ReadyCandidate],
        selected_candidate_id: &str,
        advance_manual_cursor: bool,
    ) {
        if self.mode == RotationMode::Manual {
            if advance_manual_cursor {
                if let Some(candidate) = candidates
                    .iter()
                    .find(|candidate| candidate.id == selected_candidate_id)
                {
                    self.manual_cursor.insert(
                        route_key.to_owned(),
                        ManualCursor {
                            capacity_key: candidate.capacity_key.clone(),
                            candidate_id: candidate.id.clone(),
                            priority: candidate.priority,
                        },
                    );
                }
            }
            return;
        }
        let eligible_capacity_keys: BTreeSet<_> = candidates
            .iter()
            .map(|candidate| &candidate.capacity_key)
            .collect();
        self.rotation_credit.retain(|(route, capacity), _| {
            route != route_key || eligible_capacity_keys.contains(capacity)
        });
        let total_weight = candidates
            .iter()
            .map(|candidate| i64::from(candidate.weight))
            .sum::<i64>();
        for candidate in candidates {
            let credit_key = (route_key.to_owned(), candidate.capacity_key.clone());
            let candidate_credit = self.rotation_credit.entry(credit_key).or_default();
            *candidate_credit = candidate_credit.saturating_add(i64::from(candidate.weight));
        }
        let Some(selected_candidate) = candidates
            .iter()
            .find(|candidate| candidate.id == selected_candidate_id)
        else {
            return;
        };
        let credit_key = (
            route_key.to_owned(),
            selected_candidate.capacity_key.clone(),
        );
        if let Some(candidate_credit) = self.rotation_credit.get_mut(&credit_key) {
            *candidate_credit = candidate_credit.saturating_sub(total_weight);
        }
    }
}
