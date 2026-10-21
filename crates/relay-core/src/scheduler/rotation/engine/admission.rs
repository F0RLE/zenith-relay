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
        let selection_group = self.selection_group(&ready_candidates, request, now_ms);
        let selected_candidate = self.choose_for_request(request, &selection_group)?;
        let reason = self.selection_reason(request, &ready_candidates, &selected_candidate, now_ms);
        Some(RotationSelection {
            candidate_id: selected_candidate.id.clone(),
            priority: selected_candidate.priority,
            recovery: selected_candidate.recovery,
            eligible_candidates: u32::try_from(ready_candidates.len()).unwrap_or(u32::MAX),
            reason,
        })
    }

    fn selection_reason(
        &self,
        request: &RotationRequest,
        ready_candidates: &[ReadyCandidate],
        selected: &ReadyCandidate,
        now_ms: u64,
    ) -> RotationSelectionReason {
        if request.owner.is_some() {
            return RotationSelectionReason::HardOwner;
        }
        if selected.recovery {
            return RotationSelectionReason::Recovery;
        }
        if ready_candidates.len() == 1 {
            return RotationSelectionReason::OnlyEligible;
        }
        if matches!(self.mode, RotationMode::Failover | RotationMode::RoundRobin) {
            return RotationSelectionReason::ManualPriority;
        }
        if self.mode != RotationMode::Automatic {
            return RotationSelectionReason::WeightedRotation;
        }

        // Only automatic selection compares resource headroom. Resolve its
        // explanation after selection, without scanning balances for owners,
        // recovery requests or manual/round-robin routing.
        let least_loaded = self.least_loaded_group(ready_candidates);
        if least_loaded.len() < ready_candidates.len() {
            return RotationSelectionReason::LeastLoaded;
        }
        let best_quota = least_loaded
            .iter()
            .filter_map(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms))
            .max();
        if best_quota.is_some_and(|best| {
            least_loaded
                .iter()
                .any(|candidate| !self.within_quota_band(&candidate.id, best, now_ms))
        }) {
            return RotationSelectionReason::QuotaHeadroom;
        }
        let selected_credits = self.fresh_provider_credits(&selected.id, now_ms);
        if selected_credits.is_some()
            && least_loaded
                .iter()
                .all(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms).is_none())
            && least_loaded.iter().any(|candidate| {
                self.fresh_provider_credits(&candidate.id, now_ms) != selected_credits
            })
        {
            return RotationSelectionReason::ProviderCredits;
        }
        RotationSelectionReason::WeightedRotation
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
        if self.mode == RotationMode::RoundRobin {
            return self.choose_round_robin(&request.route_key, candidate_group);
        }
        self.choose_weighted(&request.route_key, candidate_group)
    }

    fn choose_round_robin(
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

        let previous_cursor = self.round_robin_cursor.get(route_key).map(|cursor| {
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
        if runtime.candidate.quota == QuotaState::Unknown {
            return runtime
                .candidate
                .quota_reference
                .as_ref()
                .and_then(|reference| {
                    reference.fresh_remaining(now_ms, self.quota_stale_after_ms)
                });
        }
        if runtime.candidate.quota != QuotaState::Available {
            return None;
        }
        let remaining = runtime
            .candidate
            .quota_remaining_basis_points
            .filter(|remaining| *remaining > 0)?;
        if !crate::quota::observation_is_fresh(
            runtime.candidate.quota_observed_at_ms,
            now_ms,
            self.quota_stale_after_ms,
        ) {
            return None;
        }
        Some(remaining)
    }

    pub(crate) fn fresh_provider_credits(&self, candidate_id: &str, now_ms: u64) -> Option<u64> {
        let candidate = &self.candidates.get(candidate_id)?.candidate;
        if !crate::quota::observation_is_fresh(
            candidate.provider_credits_observed_at_ms,
            now_ms,
            self.quota_stale_after_ms,
        ) {
            return None;
        }
        if candidate.provider_credits_unlimited {
            return Some(u64::MAX);
        }
        candidate
            .provider_credits_micro_units
            .filter(|credits| *credits > 0)
    }

    fn within_quota_band(&self, candidate_id: &str, best: u64, now_ms: u64) -> bool {
        self.fresh_quota_remaining(candidate_id, now_ms)
            .is_some_and(|remaining| {
                best.saturating_sub(remaining) < QUOTA_SWITCH_MARGIN_BASIS_POINTS
            })
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
        if let Some(candidate) = recovery.filter(|_| self.mode != RotationMode::Failover) {
            return vec![candidate.clone()];
        }
        match self.mode {
            RotationMode::Failover => ready
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
            RotationMode::RoundRobin | RotationMode::WeightedRoundRobin => ready.to_vec(),
            RotationMode::Automatic => {
                // Spread independent concurrent work across the least-loaded
                // physical members first. Quota and credit balances decide
                // among members at the same normalized load.
                let mut candidates = self.least_loaded_group(ready);
                let best_quota = candidates
                    .iter()
                    .filter_map(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms))
                    .max();
                if let Some(best_quota) = best_quota {
                    candidates.retain(|candidate| {
                        self.within_quota_band(&candidate.id, best_quota, now_ms)
                    });
                } else if let Some(best_credits) = candidates
                    .iter()
                    .filter_map(|candidate| self.fresh_provider_credits(&candidate.id, now_ms))
                    .max()
                {
                    candidates.retain(|candidate| {
                        self.fresh_provider_credits(&candidate.id, now_ms)
                            .is_some_and(|credits| {
                                credits == best_credits
                                    || (request.preferred.as_deref() == Some(candidate.id.as_str())
                                        && best_credits.saturating_sub(credits)
                                            < PROVIDER_CREDIT_SWITCH_MARGIN_MICRO_UNITS)
                            })
                    });
                }
                self.soft_health_group(candidates)
            }
        }
    }

    fn soft_health_group(&self, mut candidates: Vec<ReadyCandidate>) -> Vec<ReadyCandidate> {
        let Some(best_failure_score) = candidates
            .iter()
            .map(|candidate| candidate.failure_score_milli)
            .min()
        else {
            return candidates;
        };
        // Compare all members with one baseline. Pairwise switch margins do
        // not define a transitive ordering and can make selection depend on IDs.
        candidates.retain(|candidate| {
            candidate
                .failure_score_milli
                .saturating_sub(best_failure_score)
                < HEALTH_FAILURE_SWITCH_MARGIN_MILLI
        });
        let fastest = candidates
            .iter()
            .filter_map(|candidate| candidate.first_output_latency_ms)
            .map(|latency| latency.max(1))
            .min();
        if let Some(fastest) = fastest {
            candidates.retain(|candidate| {
                // Missing measurements remain eligible for ordinary rotation.
                candidate.first_output_latency_ms.is_none_or(|latency| {
                    u128::from(latency.max(1)) * 1_000
                        < u128::from(fastest)
                            * u128::from(1_000 + HEALTH_LATENCY_SWITCH_MARGIN_MILLI)
                })
            });
        }
        candidates
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
            QuotaState::CreditFallback { .. }
                if crate::quota::observation_is_fresh(
                    runtime.candidate.provider_credits_observed_at_ms,
                    now_ms,
                    self.quota_stale_after_ms,
                ) => {}
            QuotaState::Exhausted {
                reset_at_ms: Some(reset_at_ms),
            }
            | QuotaState::CreditFallback {
                reset_at_ms: Some(reset_at_ms),
            } if reset_at_ms > now_ms => {
                earliest_wait = Some((reset_at_ms, CandidateBlockReason::QuotaExhausted));
            }
            QuotaState::Exhausted { .. } | QuotaState::CreditFallback { .. } => {
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
                let CandidateAvailability::Ready { recovery } =
                    self.candidate_availability(request, &runtime.candidate.id, now_ms)
                else {
                    return None;
                };
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
                    weight: if self.mode == RotationMode::RoundRobin {
                        1
                    } else {
                        runtime.candidate.weight
                    },
                    recovery,
                    due_at_ms,
                    failure_score_milli: self
                        .health
                        .get(&(runtime.candidate.id.clone(), request.route_key.clone()))
                        .map_or(0, |health| self.decayed_failure_score(health, now_ms)),
                    first_output_latency_ms: self
                        .health
                        .get(&(runtime.candidate.id.clone(), request.route_key.clone()))
                        .filter(|health| {
                            health.latency_observed_at_ms.is_some_and(|observed_at_ms| {
                                observed_at_ms <= now_ms
                                    && now_ms - observed_at_ms <= HEALTH_HALF_LIFE_MS
                            })
                        })
                        .and_then(|health| health.first_output_latency_ms),
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
                    .then_with(|| {
                        if self.mode == RotationMode::WeightedRoundRobin {
                            left.priority.cmp(&right.priority)
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
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
        advance_round_robin_cursor: bool,
    ) {
        if self.mode == RotationMode::RoundRobin {
            if advance_round_robin_cursor {
                if let Some(candidate) = candidates
                    .iter()
                    .find(|candidate| candidate.id == selected_candidate_id)
                {
                    self.round_robin_cursor.insert(
                        route_key.to_owned(),
                        RoundRobinCursor {
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
