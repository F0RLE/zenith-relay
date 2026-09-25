//! Validated members, observations and physical capacity limits.

use super::*;

impl RotationEngine {
    pub(crate) fn next_request_id() -> RequestId {
        RequestId(NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed))
    }

    pub fn mode(&self) -> RotationMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: RotationMode) {
        if self.mode != mode {
            self.rotation_credit.clear();
            self.mode = mode;
        }
    }

    pub fn set_max_in_flight(&mut self, max_in_flight: u32) -> Result<(), &'static str> {
        if max_in_flight == 0 || max_in_flight > 65_536 {
            return Err("rotation runtime capacity is invalid");
        }
        // Shrinking a limit stops new admissions, never releases active work.
        self.max_in_flight = max_in_flight;
        Ok(())
    }

    pub fn active_leases(&self) -> usize {
        self.leases.len()
    }

    /// A conservative admission fast path: no route can be selected while
    /// every physical capacity is occupied. Disabled or otherwise blocked
    /// candidates with free capacity deliberately return false here; their
    /// exact per-request eligibility still belongs to normal selection.
    pub(crate) fn all_capacity_reserved(&self) -> bool {
        if self.leases.len() >= self.max_in_flight as usize {
            return true;
        }
        self.candidates.values().all(|runtime| {
            let physical_limit = self
                .capacity_limits
                .get(&runtime.candidate.capacity_key)
                .copied()
                .unwrap_or_default();
            (physical_limit > 0
                && self.capacity_in_flight(&runtime.candidate.capacity_key) >= physical_limit)
                || (runtime.candidate.max_concurrency > 0
                    && runtime.in_flight >= runtime.candidate.max_concurrency)
        })
    }

    pub fn upsert(&mut self, mut candidate: RotationCandidate) -> Result<(), &'static str> {
        candidate.validate()?;
        let capacity_key = candidate.capacity_key.clone();
        let previous_capacity_key = self
            .candidates
            .get(&candidate.id)
            .map(|runtime| runtime.candidate.capacity_key.clone());
        let candidate_id = candidate.id.clone();
        if let Some(runtime) = self.candidates.get_mut(&candidate_id) {
            // Config edits are not provider observations. A weight/order edit
            // cannot clear auth, quota, rate or active leases.
            candidate.auth = runtime.candidate.auth;
            candidate.quota = runtime.candidate.quota;
            candidate.rate = runtime.candidate.rate;
            candidate.route_rates = runtime
                .candidate
                .route_rates
                .iter()
                .filter(|(route_key, _)| candidate.routes.contains_key(*route_key))
                .map(|(route_key, rate)| (route_key.clone(), *rate))
                .collect();
            for route_key in candidate.routes.keys() {
                candidate
                    .route_rates
                    .entry(route_key.clone())
                    .or_insert(RateState::Ready);
            }
            if candidate.weight != runtime.candidate.weight
                || candidate.capacity_key != runtime.candidate.capacity_key
                || candidate.routes != runtime.candidate.routes
            {
                self.rotation_credit.clear();
            }
            self.circuits.retain(|(id, key), _| {
                id != &candidate_id
                    || candidate.routes.get(key) == runtime.candidate.routes.get(key)
            });
            runtime.candidate = candidate;
        } else {
            let generation = self
                .next_candidate_generation
                .checked_add(1)
                .ok_or("rotation identity generation exhausted")?;
            self.next_candidate_generation = generation;
            self.candidate_generations
                .insert(candidate_id.clone(), generation);
            self.candidates.insert(
                candidate_id,
                CandidateRuntime {
                    candidate,
                    in_flight: 0,
                },
            );
        }
        self.recompute_capacity_limit(&capacity_key);
        if let Some(previous_capacity_key) = previous_capacity_key {
            if previous_capacity_key != capacity_key {
                self.recompute_capacity_limit(&previous_capacity_key);
            }
        }
        Ok(())
    }

    pub fn recovery_policy(&self) -> RecoveryPolicy {
        self.recovery_policy
    }

    pub fn set_recovery_policy(&mut self, mut policy: RecoveryPolicy) {
        policy.successful_requests_per_credit = policy.successful_requests_per_credit.max(1);
        policy.max_in_flight = policy.max_in_flight.max(1);
        self.recovery_policy = policy;
        self.recovery_credits = self.recovery_credits.min(policy.initial_credits);
    }

    pub fn recovery_credits(&self) -> u32 {
        self.recovery_credits
    }

    pub fn recovery_in_flight(&self) -> u32 {
        self.recovery_in_flight
    }

    pub fn candidate(&self, candidate_id: &str) -> Option<&RotationCandidate> {
        self.candidates
            .get(candidate_id)
            .map(|runtime| &runtime.candidate)
    }

    pub(crate) fn candidate_ids(&self) -> impl Iterator<Item = String> + '_ {
        self.candidates.keys().cloned()
    }

    /// Applies owner-local observations to the rotation registry without touching
    /// leases, circuit generations, weighted history, or response ownership.
    pub fn sync_candidate_state(
        &mut self,
        candidate_id: &str,
        enabled: bool,
        draining: bool,
        auth: AuthState,
        quota: QuotaState,
        rate: RateState,
    ) -> bool {
        let Some(runtime) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        runtime.candidate.enabled = enabled;
        runtime.candidate.draining = draining;
        runtime.candidate.auth = auth;
        runtime.candidate.quota = quota;
        runtime.candidate.rate = rate;
        true
    }

    /// Applies a cooldown observation to one exact model/operation route.
    /// Global rate state remains separate and is used only when a route has no
    /// more specific observation.
    pub fn sync_candidate_route_rate(
        &mut self,
        candidate_id: &str,
        route_key: &str,
        rate: RateState,
    ) -> bool {
        let Some(runtime) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        runtime
            .candidate
            .route_rates
            .insert(route_key.to_owned(), rate);
        true
    }

    /// Removes a candidate from future admission while leaving existing lease
    /// identities valid until their owner settles.  The lease stores its
    /// physical capacity key, so a remove-and-readd cannot release the new
    /// candidate's reservation by accident.
    pub fn remove(&mut self, candidate_id: &str) -> Option<RotationCandidate> {
        let runtime = self.candidates.remove(candidate_id)?;
        self.candidate_generations.remove(candidate_id);
        self.quota_revisions.remove(candidate_id);
        self.circuits
            .retain(|(candidate, _), _| candidate != candidate_id);
        let capacity_key = runtime.candidate.capacity_key.clone();
        self.recompute_capacity_limit(&capacity_key);
        self.rotation_credit.clear();
        Some(runtime.candidate)
    }

    pub(super) fn recompute_capacity_limit(&mut self, capacity_key: &str) {
        let mut members = self
            .candidates
            .values()
            .filter(|candidate| candidate.candidate.capacity_key == capacity_key)
            .peekable();
        if members.peek().is_none() {
            self.capacity_limits.remove(capacity_key);
            return;
        }
        let limit = members
            .flat_map(|runtime| {
                [
                    runtime.candidate.capacity_limit,
                    runtime.candidate.max_concurrency,
                ]
            })
            .filter(|limit| *limit > 0)
            .min()
            .unwrap_or_default();
        self.capacity_limits.insert(capacity_key.to_owned(), limit);
    }

    pub fn in_flight(&self, candidate_id: &str) -> u32 {
        self.candidates
            .get(candidate_id)
            .map_or(0, |runtime| runtime.in_flight)
    }

    pub fn capacity_in_flight(&self, capacity_key: &str) -> u32 {
        self.capacity_in_flight
            .get(capacity_key)
            .copied()
            .unwrap_or_default()
    }

    pub fn set_auth(&mut self, candidate_id: &str, auth: AuthState) -> bool {
        self.candidates
            .get_mut(candidate_id)
            .map(|runtime| runtime.candidate.auth = auth)
            .is_some()
    }

    pub fn set_quota(&mut self, candidate_id: &str, quota: QuotaState) -> bool {
        let revision = self
            .quota_revisions
            .get(candidate_id)
            .copied()
            .unwrap_or_default()
            .saturating_add(1);
        self.set_quota_if_newer(candidate_id, quota, revision)
    }

    /// Applies a provider observation only when its fence is newer than the
    /// observation already installed for this candidate.  In particular, a
    /// positive cached read that races a newer 429 cannot reopen a route.
    pub fn set_quota_if_newer(
        &mut self,
        candidate_id: &str,
        quota: QuotaState,
        revision: u64,
    ) -> bool {
        let current = self
            .quota_revisions
            .get(candidate_id)
            .copied()
            .unwrap_or_default();
        if revision <= current {
            return false;
        }
        let Some(runtime) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        runtime.candidate.quota = quota;
        self.quota_revisions
            .insert(candidate_id.to_owned(), revision);
        true
    }

    pub fn set_rate(&mut self, candidate_id: &str, rate: RateState) -> bool {
        self.candidates
            .get_mut(candidate_id)
            .map(|runtime| runtime.candidate.rate = rate)
            .is_some()
    }

    pub fn set_enabled(&mut self, candidate_id: &str, enabled: bool) -> bool {
        self.candidates
            .get_mut(candidate_id)
            .map(|runtime| runtime.candidate.enabled = enabled)
            .is_some()
    }
}
