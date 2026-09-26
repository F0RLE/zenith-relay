//! Member observations, configuration updates and draining removal.

use super::*;

impl PoolScheduler {
    pub fn set_quota_stale_after_ms(&mut self, stale_after_ms: u64) {
        self.quota_stale_after_ms = stale_after_ms.max(1);
    }

    pub fn upsert(&mut self, candidate: RuntimeCandidate) {
        let candidate_id = candidate.id.clone();
        if self.candidates.get(&candidate_id).is_some_and(|previous| {
            previous.enabled != candidate.enabled
                || previous.draining != candidate.draining
                || previous.secret_available != candidate.secret_available
                || previous.kind != candidate.kind
                || previous.source_id != candidate.source_id
                || previous.account_id != candidate.account_id
                || previous.protocol != candidate.protocol
                || previous.model_rules != candidate.model_rules
                || previous
                    .models
                    .difference(&candidate.models)
                    .next()
                    .is_some()
        }) {
            let revision = self
                .candidate_permission_revisions
                .entry(candidate_id.clone())
                .or_default();
            *revision = revision
                .checked_add(1)
                .expect("candidate permission revision exhausted");
        }
        // A newly reintroduced id must not inherit an opaque response owner
        // from a previously deleted credential or source. Existing candidates
        // are updated in place and keep their conversation affinity.
        if !self.candidates.contains_key(&candidate_id) {
            self.response_affinity.invalidate_candidate(&candidate_id);
        }
        self.retired_candidates.remove(&candidate_id);
        self.cooldown_reasons.retain(|(id, model), _| {
            id != &candidate_id || candidate.cooldowns.contains_key(model)
        });
        self.candidates.insert(candidate.id.clone(), candidate);
    }

    pub fn remove(&mut self, candidate_id: &str) -> Option<RuntimeCandidate> {
        let existing = self.candidates.get(candidate_id).cloned()?;
        // The rotation engine retains old leases itself. Retire its identity now,
        // not after drain, so a same-id re-add cannot inherit a late outcome.
        self.rotation.remove(candidate_id);
        if self.active_request_count(candidate_id) > 0 {
            // Do not tear down activity or executor ownership underneath an
            // in-flight request. The candidate is immediately ineligible for
            // new work and is finalized once its last lease is released.
            // Keep response affinity until the request layer can consume a
            // bounded native replay for a continuation arriving after this
            // candidate is removed. The retired candidate remains ineligible
            // for new routing.
            self.prompt_affinity.invalidate_candidate(candidate_id);
            if let Some(candidate) = self.candidates.get_mut(candidate_id) {
                candidate.enabled = false;
                candidate.draining = true;
            }
            self.retired_candidates.insert(candidate_id.to_string());
            return Some(existing);
        }
        self.remove_now(candidate_id)
    }

    pub(super) fn remove_now(&mut self, candidate_id: &str) -> Option<RuntimeCandidate> {
        self.rotation.remove(candidate_id);
        self.retired_candidates.remove(candidate_id);
        // Retain the short-lived response owner so a pending continuation can
        // load its bounded native replay and hand off to another candidate.
        self.prompt_affinity.invalidate_candidate(candidate_id);
        self.activity.remove_candidate(candidate_id);
        self.reservations.remove_candidate(candidate_id);
        self.execution_fences.remove(candidate_id);
        self.candidate_permission_revisions.remove(candidate_id);
        self.capability_blocks
            .retain(|(blocked_candidate, _)| blocked_candidate != candidate_id);
        self.cooldown_reasons
            .retain(|(cooled_candidate, _), _| cooled_candidate != candidate_id);
        if self
            .protected_candidate
            .as_ref()
            .is_some_and(|(protected_id, _)| protected_id == candidate_id)
        {
            self.protected_candidate = None;
        }
        let removed = self.candidates.remove(candidate_id);
        if let Some(candidate) = &removed {
            let key = members::member_key(candidate);
            if !self
                .candidates
                .values()
                .any(|other| members::member_key(other) == key)
            {
                self.member_activity.remove_candidate(&key);
            }
        }
        removed
    }

    pub(super) fn finalize_retired_if_idle(&mut self, candidate_id: &str) {
        if self.retired_candidates.contains(candidate_id)
            && self.active_request_count(candidate_id) == 0
        {
            let _ = self.remove_now(candidate_id);
        }
    }

    pub fn candidate(&self, candidate_id: &str) -> Option<&RuntimeCandidate> {
        self.candidates.get(candidate_id)
    }

    pub(crate) fn candidate_permission_revision(&self, candidate_id: &str) -> u64 {
        self.candidate_permission_revisions
            .get(candidate_id)
            .copied()
            .unwrap_or_default()
    }

    pub fn set_protected_candidate(
        &mut self,
        candidate_id: Option<&str>,
        reserve_basis_points: u64,
    ) -> bool {
        let Some(candidate_id) = candidate_id else {
            self.protected_candidate = None;
            return true;
        };
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.protected_candidate = Some((candidate_id.to_string(), reserve_basis_points));
        true
    }

    pub fn candidates(&self) -> impl Iterator<Item = &RuntimeCandidate> {
        self.candidates.values()
    }

    pub fn update_candidate_availability(
        &mut self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
    ) -> bool {
        self.update_candidate_availability_at(candidate_id, enabled, health, quota, None)
    }

    pub fn update_candidate_availability_at(
        &mut self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota: CandidateQuota,
        quota_updated_at_ms: Option<u64>,
    ) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        let quota_changed = candidate.quota != quota;
        let quota_timestamp_changed =
            quota_updated_at_ms.is_some() && candidate.quota_updated_at_ms != quota_updated_at_ms;
        candidate.enabled = enabled;
        candidate.health = health;
        candidate.quota = quota;
        if quota_changed || quota_timestamp_changed {
            candidate.quota_updated_at_ms = quota_updated_at_ms;
        }
        true
    }

    /// Updates the operational and quota state from one fresh account snapshot
    /// while holding the scheduler lock. Keeping these fields together avoids
    /// dispatching with a newly refreshed quota and stale provider credits.
    pub fn update_candidate_availability_with_quota_at(
        &mut self,
        candidate_id: &str,
        enabled: bool,
        health: CandidateHealth,
        quota_state: CandidateQuotaState,
    ) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.enabled = enabled;
        candidate.health = health;
        candidate.quota = quota_state.quota;
        candidate.quota_updated_at_ms = quota_state.updated_at_ms;
        candidate.quota_reset_at_ms = quota_state.reset_at_ms;
        candidate.provider_credits_micro_units = quota_state.provider_credits_micro_units;
        candidate.provider_credits_unlimited = quota_state.provider_credits_unlimited;
        true
    }

    pub fn update_candidate_quota_at(
        &mut self,
        candidate_id: &str,
        quota: CandidateQuota,
        quota_updated_at_ms: Option<u64>,
        quota_reset_at_ms: Option<u64>,
        provider_credits_micro_units: Option<u64>,
        provider_credits_unlimited: bool,
    ) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.quota = quota;
        candidate.quota_updated_at_ms = quota_updated_at_ms;
        candidate.quota_reset_at_ms = quota_reset_at_ms;
        candidate.provider_credits_micro_units = provider_credits_micro_units;
        candidate.provider_credits_unlimited = provider_credits_unlimited;
        true
    }

    pub fn set_execution_fence(&mut self, candidate_id: &str, fenced: bool) -> bool {
        if fenced {
            return self.begin_execution_fence(candidate_id).is_some();
        }
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        if let Some((epoch, _)) = self.execution_fences.get(candidate_id).copied() {
            self.end_execution_fence(candidate_id, epoch);
        }
        true
    }

    pub(crate) fn begin_execution_fence(&mut self, candidate_id: &str) -> Option<u64> {
        self.candidates.get(candidate_id)?;
        if let Some((epoch, count)) = self.execution_fences.get_mut(candidate_id) {
            *count = count
                .checked_add(1)
                .expect("execution fence count exhausted");
            return Some(*epoch);
        }
        self.next_execution_fence_epoch = self
            .next_execution_fence_epoch
            .checked_add(1)
            .expect("execution fence epoch exhausted");
        let epoch = self.next_execution_fence_epoch;
        self.execution_fences
            .insert(candidate_id.to_string(), (epoch, 1));
        Some(epoch)
    }

    pub(crate) fn end_execution_fence(&mut self, candidate_id: &str, epoch: u64) {
        let Some((current_epoch, count)) = self.execution_fences.get_mut(candidate_id) else {
            return;
        };
        if *current_epoch != epoch {
            // A removed and re-added candidate owns a different fence group.
            return;
        }
        if *count <= 1 {
            self.execution_fences.remove(candidate_id);
        } else {
            *count -= 1;
        }
    }

    pub fn block_capability(&mut self, candidate_id: &str, model: &str) -> bool {
        self.candidates.contains_key(candidate_id)
            && self
                .capability_blocks
                .insert((candidate_id.to_string(), model.to_ascii_lowercase()))
    }

    pub fn clear_capability_blocks(&mut self, candidate_id: &str) -> bool {
        let previous = self.capability_blocks.len();
        self.capability_blocks
            .retain(|(blocked_candidate, _)| blocked_candidate != candidate_id);
        self.capability_blocks.len() != previous
    }

    pub fn set_candidate_health(&mut self, candidate_id: &str, health: CandidateHealth) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.health = health;
        true
    }

    pub(super) fn quota_reserve_allows(&self, candidate: &RuntimeCandidate, now_ms: u64) -> bool {
        let Some((_, reserve)) = self
            .protected_candidate
            .as_ref()
            .filter(|(candidate_id, _)| candidate_id == &candidate.id)
        else {
            return true;
        };
        if matches!(candidate.quota, CandidateQuota::Available(_))
            && candidate.quota_updated_at_ms.is_some_and(|updated_at_ms| {
                now_ms.saturating_sub(updated_at_ms) > self.quota_stale_after_ms
            })
        {
            return false;
        }
        matches!(candidate.quota, CandidateQuota::Available(remaining) if remaining > *reserve)
    }
}
