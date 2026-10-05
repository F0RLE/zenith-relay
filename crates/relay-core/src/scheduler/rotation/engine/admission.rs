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
        let (ready, _) = self.selection_candidates(request, now_ms);
        let least_loaded = self.least_loaded_group(&ready);
        let group = self.selection_group(&ready, request, now_ms);
        let selected = self.choose_for_request(request, &group)?;
        let selected_quota = self.fresh_quota_remaining(&selected.id, now_ms);
        let quota_decided = request.owner.is_none()
            && !selected.recovery
            && self.mode == RotationMode::Automatic
            && selected_quota.is_some()
            && least_loaded.iter().any(|candidate| {
                self.fresh_quota_remaining(&candidate.id, now_ms) != selected_quota
            });
        let selected_credits = self.fresh_provider_credits(&selected.id, now_ms);
        let credits_decided = request.owner.is_none()
            && !selected.recovery
            && self.mode == RotationMode::Automatic
            && least_loaded
                .iter()
                .all(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms).is_none())
            && selected_credits.is_some()
            && least_loaded.iter().any(|candidate| {
                self.fresh_provider_credits(&candidate.id, now_ms) != selected_credits
            });
        let load_decided = least_loaded.len() < ready.len();
        let reason = if request.owner.is_some() {
            RotationSelectionReason::HardOwner
        } else if selected.recovery {
            RotationSelectionReason::Recovery
        } else if ready.len() == 1 {
            RotationSelectionReason::OnlyEligible
        } else if matches!(self.mode, RotationMode::InOrder | RotationMode::Manual) {
            RotationSelectionReason::ManualPriority
        } else if load_decided && self.mode == RotationMode::Automatic {
            RotationSelectionReason::LeastLoaded
        } else if quota_decided {
            RotationSelectionReason::QuotaHeadroom
        } else if credits_decided {
            RotationSelectionReason::ProviderCredits
        } else {
            RotationSelectionReason::WeightedRotation
        };
        Some(RotationSelection {
            candidate_id: selected.id.clone(),
            priority: selected.priority,
            recovery: selected.recovery,
            eligible_candidates: u32::try_from(ready.len()).unwrap_or(u32::MAX),
            reason,
        })
    }

    pub(super) fn selection_candidates(
        &self,
        request: &RotationRequest,
        now_ms: u64,
    ) -> (Vec<ReadyCandidate>, bool) {
        let ready = self.ready_candidates(request, now_ms);
        let ordinary_alternatives = ready.iter().any(|candidate| !candidate.recovery);
        let recovery_allowed = self.recovery_in_flight < self.recovery_policy.max_in_flight
            && (!ordinary_alternatives || self.recovery_credits > 0);
        let ready = ready
            .into_iter()
            .filter(|candidate| !candidate.recovery || recovery_allowed)
            .collect::<Vec<_>>();
        (ready, ordinary_alternatives)
    }

    pub(super) fn choose_for_request(
        &self,
        request: &RotationRequest,
        group: &[ReadyCandidate],
    ) -> Option<ReadyCandidate> {
        if self.mode == RotationMode::Automatic && request.owner.is_none() {
            if let Some(preferred) = group.iter().find(|candidate| {
                !candidate.recovery && request.preferred.as_deref() == Some(&candidate.id)
            }) {
                return Some(preferred.clone());
            }
        }
        if self.mode == RotationMode::Manual {
            return self.choose_manual(&request.route_key, group);
        }
        self.choose_weighted(&request.route_key, group)
    }

    fn choose_manual(
        &self,
        route_key: &str,
        candidates: &[ReadyCandidate],
    ) -> Option<ReadyCandidate> {
        let mut ordered = candidates.iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            right
                .priority
                .cmp(&left.priority)
                .then_with(|| left.id.cmp(&right.id))
        });
        if ordered.is_empty() {
            return None;
        }

        let cursor = self.manual_cursor.get(route_key).map(|cursor| {
            let priority = self
                .candidates
                .values()
                .find(|runtime| runtime.candidate.capacity_key == cursor.capacity_key)
                .map_or(cursor.priority, |runtime| runtime.candidate.priority);
            (priority, cursor.candidate_id.as_str())
        });
        let next = cursor
            .and_then(|(priority, candidate_id)| {
                ordered.iter().position(|candidate| {
                    candidate.priority < priority
                        || (candidate.priority == priority && candidate.id.as_str() > candidate_id)
                })
            })
            .unwrap_or(0);
        ordered.get(next).map(|candidate| (*candidate).clone())
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

    fn least_loaded_group(&self, ready: &[ReadyCandidate]) -> Vec<ReadyCandidate> {
        let compare_load = |left: &ReadyCandidate, right: &ReadyCandidate| {
            (u64::from(left.in_flight) * u64::from(right.effective_capacity))
                .cmp(&(u64::from(right.in_flight) * u64::from(left.effective_capacity)))
        };
        let Some(best) = ready.iter().min_by(|left, right| compare_load(left, right)) else {
            return Vec::new();
        };
        ready
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
                let least_loaded = self.least_loaded_group(ready);
                let best_quota = least_loaded
                    .iter()
                    .filter_map(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms))
                    .max();
                if let Some(best_quota) = best_quota {
                    return least_loaded
                        .iter()
                        .filter(|candidate| {
                            self.fresh_quota_remaining(&candidate.id, now_ms) == Some(best_quota)
                        })
                        .cloned()
                        .collect();
                }

                let best_credits = least_loaded
                    .iter()
                    .filter_map(|candidate| self.fresh_provider_credits(&candidate.id, now_ms))
                    .max();
                if let Some(best_credits) = best_credits {
                    if let Some(preferred) = least_loaded.iter().find(|candidate| {
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
                    return least_loaded
                        .iter()
                        .filter(|candidate| {
                            self.fresh_provider_credits(&candidate.id, now_ms) == Some(best_credits)
                        })
                        .cloned()
                        .collect();
                }
                least_loaded
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
        let mut wait = None;
        match runtime.candidate.quota {
            QuotaState::Exhausted {
                reset_at_ms: Some(reset_at_ms),
            } if reset_at_ms > now_ms => {
                wait = Some((reset_at_ms, CandidateBlockReason::QuotaExhausted));
            }
            QuotaState::Exhausted { .. } => {
                return CandidateAvailability::Blocked(CandidateBlockReason::QuotaExhausted)
            }
            QuotaState::Unknown | QuotaState::Available | QuotaState::Stale => {}
        }
        let route_rate = runtime
            .candidate
            .route_rates
            .get(route_key)
            .copied()
            .unwrap_or(runtime.candidate.rate);
        for rate in [runtime.candidate.rate, route_rate] {
            if let RateState::Limited { not_before_ms } = rate {
                if not_before_ms > now_ms && wait.is_none_or(|(at, _)| not_before_ms > at) {
                    wait = Some((not_before_ms, CandidateBlockReason::RateLimited));
                }
            }
        }
        let circuit = self
            .circuits
            .get(&(runtime.candidate.id.clone(), route_key.to_owned()))
            .cloned()
            .unwrap_or_default();
        if let Some(at) = circuit.not_before_ms.filter(|at| *at > now_ms) {
            if wait.is_none_or(|(previous, _)| at > previous) {
                wait = Some((at, CandidateBlockReason::CircuitOpen));
            }
        }
        if let Some((at_ms, reason)) = wait {
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
        match circuit.state {
            CircuitState::Closed => {}
            CircuitState::Degraded | CircuitState::Open => {
                if circuit.half_open_lease.is_some() {
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
                |mut members, candidate| {
                    members
                        .entry(candidate.capacity_key.clone())
                        .or_insert(candidate);
                    members
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
                let left_credit = self
                    .rotation_credit
                    .get(&(route_key.to_owned(), left.capacity_key.clone()))
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(i64::from(left.weight));
                let right_credit = self
                    .rotation_credit
                    .get(&(route_key.to_owned(), right.capacity_key.clone()))
                    .copied()
                    .unwrap_or_default()
                    .saturating_add(i64::from(right.weight));
                left_credit
                    .cmp(&right_credit)
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
        selected: &str,
        advance_manual_cursor: bool,
    ) {
        if self.mode == RotationMode::Manual {
            if advance_manual_cursor {
                if let Some(candidate) =
                    candidates.iter().find(|candidate| candidate.id == selected)
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
        let eligible: BTreeSet<_> = candidates
            .iter()
            .map(|candidate| &candidate.capacity_key)
            .collect();
        self.rotation_credit
            .retain(|(route, capacity), _| route != route_key || eligible.contains(capacity));
        let total_weight = candidates
            .iter()
            .map(|candidate| i64::from(candidate.weight))
            .sum::<i64>();
        for candidate in candidates {
            let key = (route_key.to_owned(), candidate.capacity_key.clone());
            let credit = self.rotation_credit.entry(key).or_default();
            *credit = credit.saturating_add(i64::from(candidate.weight));
        }
        let Some(selected) = candidates.iter().find(|candidate| candidate.id == selected) else {
            return;
        };
        let key = (route_key.to_owned(), selected.capacity_key.clone());
        if let Some(credit) = self.rotation_credit.get_mut(&key) {
            *credit = credit.saturating_sub(total_weight);
        }
    }
}
