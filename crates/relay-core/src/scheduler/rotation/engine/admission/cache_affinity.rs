use super::*;

impl RotationEngine {
    /// Read the same admission policy before reservation. No identifiers, keys,
    /// prompts or credentials are retained, and no rotation state is advanced.
    pub(crate) fn cache_affinity_diagnostics(
        &self,
        request: &RotationRequest,
        preferred: Option<&str>,
        selected: &str,
        now_ms: u64,
    ) -> Option<CacheAffinityDiagnostics> {
        let selected_member = self.cache_affinity_member(selected, now_ms)?;
        let mut diagnostics = CacheAffinityDiagnostics {
            selection: if preferred == Some(selected) {
                CacheAffinitySelection::Preferred
            } else if request.warm_fallbacks.iter().any(|id| id == selected) {
                CacheAffinitySelection::WarmAlternative
            } else {
                CacheAffinitySelection::Other
            },
            reason: CacheAffinityReason::NoBinding,
            owner_block_reason: None,
            preferred: preferred.and_then(|id| self.cache_affinity_member(id, now_ms)),
            selected: selected_member,
            best_quota_remaining_basis_points: None,
            best_provider_credits_micro_units: None,
            best_provider_credits_unlimited: false,
            quota_switch_margin_basis_points: preferred
                .map_or(QUOTA_SWITCH_MARGIN_BASIS_POINTS, |id| {
                    self.quota_switch_margin(request, id)
                }),
            provider_credit_switch_margin_micro_units: PROVIDER_CREDIT_SWITCH_MARGIN_MICRO_UNITS,
            load_allowance_requests: CACHE_AFFINITY_EXTRA_REQUESTS,
        };
        let Some(preferred) = preferred else {
            return Some(diagnostics);
        };
        diagnostics.reason = if preferred == selected {
            CacheAffinityReason::Retained
        } else if request.owner.is_some() {
            CacheAffinityReason::ResponseOwner
        } else if self.mode != RotationMode::Automatic {
            CacheAffinityReason::RoutingPolicy
        } else {
            match self.candidate_availability(request, preferred, now_ms) {
                CandidateAvailability::Blocked(reason)
                | CandidateAvailability::Busy(reason)
                | CandidateAvailability::WaitUntil { reason, .. } => {
                    diagnostics.owner_block_reason = Some(reason);
                    CacheAffinityReason::OwnerUnavailable
                }
                CandidateAvailability::Ready { recovery: true } => {
                    CacheAffinityReason::RecoveryProbe
                }
                CandidateAvailability::Ready { recovery: false } => {
                    self.cache_affinity_filter_reason(request, preferred, now_ms, &mut diagnostics)
                }
            }
        };
        Some(diagnostics)
    }

    fn cache_affinity_filter_reason(
        &self,
        request: &RotationRequest,
        preferred: &str,
        now_ms: u64,
        diagnostics: &mut CacheAffinityDiagnostics,
    ) -> CacheAffinityReason {
        let (ready, _) = self.selection_candidates(request, now_ms);
        if ready.iter().any(|candidate| candidate.recovery) {
            return CacheAffinityReason::RecoveryProbe;
        }
        let load_group = self.cache_aware_load_group(&ready, request);
        diagnostics.best_quota_remaining_basis_points = load_group
            .iter()
            .filter_map(|candidate| self.fresh_quota_remaining(&candidate.id, now_ms))
            .max();
        let credits = load_group
            .iter()
            .filter_map(|candidate| self.fresh_provider_credits(&candidate.id, now_ms))
            .max();
        diagnostics.best_provider_credits_unlimited = credits == Some(u64::MAX);
        diagnostics.best_provider_credits_micro_units = credits.filter(|value| *value != u64::MAX);
        if !load_group.iter().any(|candidate| candidate.id == preferred) {
            return CacheAffinityReason::ParallelLoad;
        }
        let resources = self.resource_group(load_group, request, now_ms);
        if !resources.iter().any(|candidate| candidate.id == preferred) {
            return if diagnostics.best_quota_remaining_basis_points.is_some() {
                CacheAffinityReason::QuotaHeadroom
            } else {
                CacheAffinityReason::ProviderCredits
            };
        }
        if !self
            .soft_health_group(resources)
            .iter()
            .any(|candidate| candidate.id == preferred)
        {
            return CacheAffinityReason::Health;
        }
        // A structurally projected alias may differ from the recorded owner.
        // Never invent a resource or provider cache-miss reason in that case.
        CacheAffinityReason::RoutingPolicy
    }

    fn cache_affinity_member(&self, id: &str, now_ms: u64) -> Option<CacheAffinityMember> {
        let runtime = self.candidates.get(id)?;
        let credits = self.fresh_provider_credits(id, now_ms);
        Some(CacheAffinityMember {
            quota_remaining_basis_points: self.fresh_quota_remaining(id, now_ms),
            provider_credits_micro_units: credits.filter(|value| *value != u64::MAX),
            provider_credits_unlimited: credits == Some(u64::MAX),
            in_flight: self.capacity_in_flight(&runtime.candidate.capacity_key),
            effective_capacity: self
                .capacity_limits
                .get(&runtime.candidate.capacity_key)
                .copied()
                .filter(|limit| *limit > 0)
                .unwrap_or(self.max_in_flight)
                .min(self.max_in_flight),
        })
    }
}
