//! Configured route and request projections for rotation admission.

use super::*;

impl PoolScheduler {
    pub(super) fn sync_rotation_mode(&mut self) {
        let mode = self
            .pool_routing
            .as_ref()
            .map(|policy| match policy.mode {
                crate::PoolRoutingMode::Automatic => RotationMode::Automatic,
                crate::PoolRoutingMode::InOrder => RotationMode::InOrder,
                crate::PoolRoutingMode::RoundRobin => RotationMode::RoundRobin,
                // `set_pool_routing` validates activation before storing the
                // policy. Smart is therefore a storage/import compatibility
                // value only and must never silently select a runtime mode.
                crate::PoolRoutingMode::Smart => {
                    unreachable!("legacy rotation policy reached active scheduler")
                }
            })
            .unwrap_or(RotationMode::Automatic);
        self.rotation.set_mode(mode);
    }

    pub(super) fn rotation_route_key(model: &str, operation: RotationOperation) -> String {
        format!("{}:{operation:?}", model.to_ascii_lowercase())
    }

    pub(super) fn rotation_candidate(
        &self,
        candidate: &RuntimeCandidate,
    ) -> Option<RotationCandidate> {
        let first_model = candidate.models.iter().next()?.clone();
        let first_key = Self::rotation_route_key(&first_model, RotationOperation::Text);
        let mut result = RotationCandidate::new(&candidate.id, first_key, first_model);
        result.capacity_key = members::member_key(candidate);
        result.priority = self
            .member_policy(candidate)
            .map_or(candidate.priority, |(rank, _)| {
                -i32::try_from(rank).unwrap_or(i32::MAX)
            });
        result.weight = self.member_weight(candidate).clamp(1, 100);
        result.capacity_limit = self
            .member_policy(candidate)
            .and_then(|(_, member)| (member.max_concurrency > 0).then_some(member.max_concurrency))
            .unwrap_or_default();
        result.enabled = candidate.enabled;
        result.draining = candidate.draining;
        result.routes.clear();
        result.route_rates.clear();
        for model in &candidate.models {
            if crate::runtime::is_image_model_id(model) {
                result.add_route(
                    Self::rotation_route_key(model, RotationOperation::Image),
                    model,
                    RotationOperation::Image,
                );
                continue;
            }
            result.add_route(
                Self::rotation_route_key(model, RotationOperation::Text),
                model,
                RotationOperation::Text,
            );
            if candidate.kind == CandidateKind::OAuthAccount
                && candidate.protocol == WireApi::Responses
            {
                result.add_route(
                    Self::rotation_route_key(model, RotationOperation::Compaction),
                    model,
                    RotationOperation::Compaction,
                );
            }
        }
        Some(result)
    }

    pub(super) fn sync_all_rotation_candidates(&mut self) {
        let current_ids = self.candidates.keys().cloned().collect::<BTreeSet<_>>();
        let stale_ids = self
            .rotation_leases
            .values()
            .map(|lease| lease.candidate_id.clone())
            .collect::<BTreeSet<_>>();
        for candidate in self.candidates.values() {
            if self.retired_candidates.contains(&candidate.id) {
                continue;
            }
            let Some(rotation_candidate) = self.rotation_candidate(candidate) else {
                continue;
            };
            let id = rotation_candidate.id.clone();
            let _ = self.rotation.upsert(rotation_candidate);
            let auth = if candidate.health.is_eligible() {
                RotationAuthState::Ready
            } else {
                RotationAuthState::Blocked
            };
            let quota = match self.routing_quota(candidate) {
                CandidateQuota::Unknown => RotationQuotaState::Unknown,
                CandidateQuota::Available(0) => RotationQuotaState::Exhausted {
                    reset_at_ms: candidate.quota_reset_at_ms,
                },
                CandidateQuota::Available(_) => RotationQuotaState::Available,
                CandidateQuota::Stale => RotationQuotaState::Stale,
                CandidateQuota::Exhausted => RotationQuotaState::Exhausted {
                    reset_at_ms: candidate.quota_reset_at_ms,
                },
            };
            let global_not_before_ms = candidate.cooldowns.get("*").copied().filter(|_| {
                self.cooldown_reasons.get(&(id.clone(), "*".into()))
                    != Some(&CooldownReason::Transient)
            });
            let rate = global_not_before_ms.map_or(RotationRateState::Ready, |not_before_ms| {
                RotationRateState::Limited { not_before_ms }
            });
            let _ = self.rotation.sync_candidate_state(
                &id,
                candidate.enabled,
                candidate.draining,
                auth,
                quota,
                rate,
            );
            for model in &candidate.models {
                let not_before_ms = candidate
                    .cooldowns
                    .iter()
                    .filter(|(scope, _)| scope.as_str() == "*" || scope.eq_ignore_ascii_case(model))
                    .filter(|(scope, _)| {
                        self.cooldown_reasons.get(&(id.clone(), (*scope).clone()))
                            != Some(&CooldownReason::Transient)
                    })
                    .map(|(_, not_before_ms)| *not_before_ms)
                    .max();
                let rate = not_before_ms.map_or(RotationRateState::Ready, |not_before_ms| {
                    RotationRateState::Limited { not_before_ms }
                });
                for operation in [
                    RotationOperation::Text,
                    RotationOperation::Compaction,
                    RotationOperation::Image,
                ] {
                    let route_key = Self::rotation_route_key(model, operation);
                    let _ = self
                        .rotation
                        .sync_candidate_route_rate(&id, &route_key, rate);
                }
            }
        }
        for stale in self
            .rotation
            .candidate_ids()
            .filter(|id| !current_ids.contains(id) && !stale_ids.contains(id))
            .collect::<Vec<_>>()
        {
            let _ = self.rotation.remove(&stale);
        }
    }

    pub(super) fn rotation_request(
        &self,
        request_id: Option<RotationRequestId>,
        candidate_id: Option<&str>,
        model: &str,
        operation: RotationOperation,
        allowed: BTreeSet<String>,
    ) -> RotationRequest {
        let mut request = RotationRequest::new(
            request_id.unwrap_or_else(RotationEngine::next_request_id),
            Self::rotation_route_key(model, operation),
            model,
        )
        .with_operation(operation)
        .with_allowed_candidates(allowed);
        if let Some(candidate_id) = candidate_id {
            request = request.with_owner(candidate_id);
        }
        request
    }

    pub(super) fn prepare_rotation_request(
        &mut self,
        request: &SelectionRequest<'_>,
        operation: RotationOperation,
    ) -> Option<RotationRequest> {
        if self.retired {
            return None;
        }
        self.sync_all_rotation_candidates();
        let mut allowed = BTreeSet::new();
        for candidate in self.candidates.values() {
            if request.tried.contains(&candidate.id)
                || !self.rotation_visible(
                    candidate,
                    request.model,
                    request.allowed_protocols,
                    request.scope,
                    request.now_ms,
                )
            {
                continue;
            }
            allowed.insert(candidate.id.clone());
        }
        let owner = request.response_affinity_key.and_then(|key| {
            self.response_affinity
                .get(key, request.now_ms)
                .map(str::to_owned)
        });
        if owner
            .as_deref()
            .is_some_and(|candidate_id| !allowed.contains(candidate_id))
        {
            return None;
        }
        // One physical member has one vote; prefer its native protocol route
        // before engine selection so aliases cannot alter weight or capacity.
        let mut members = BTreeMap::new();
        for id in &allowed {
            let candidate = &self.candidates[id];
            let entry = members
                .entry(members::member_key(candidate))
                .or_insert(candidate);
            if self.compare_member_routes(candidate, entry).is_gt() {
                *entry = candidate;
            }
        }
        if owner.is_none() {
            allowed = members
                .into_values()
                .map(|candidate| candidate.id.clone())
                .collect();
        }
        let mut rotation_request =
            self.rotation_request(None, owner.as_deref(), request.model, operation, allowed);
        rotation_request.preferred = request.prompt_affinity_key.and_then(|key| {
            self.prompt_affinity
                .get(key, request.now_ms)
                .map(str::to_owned)
        });
        Some(rotation_request)
    }

    /// Structural eligibility. Mutable auth, quota, rate and circuits belong
    /// to RotationEngine, including their deadlines and recovery permits.
    pub(super) fn rotation_visible(
        &self,
        candidate: &RuntimeCandidate,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        !self.retired
            && !self.retired_candidates.contains(&candidate.id)
            && !self.execution_fences.contains_key(&candidate.id)
            && !self
                .capability_blocks
                .contains(&(candidate.id.clone(), model.to_ascii_lowercase()))
            && self.quota_reserve_allows(candidate, now_ms)
            && candidate.is_configured(model, allowed_protocols, scope)
    }
}
