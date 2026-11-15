use super::*;

impl RotationEngine {
    /// Inspect the same admission gates and selector used by dispatch. This
    /// method reads a fixed snapshot; it acquires no leases or recovery permits.
    pub fn decision_trace(&self, request: &RotationRequest, now_ms: u64) -> RotationDecisionTrace {
        let physical = self.ready_candidates(request, now_ms);
        let (ready, _) = self.selection_candidates(request, now_ms);
        let mut candidates = self
            .candidates
            .values()
            .map(|runtime| self.candidate_decision(&runtime.candidate, request, now_ms))
            .collect::<Vec<_>>();
        let mut stages = vec![RotationDecisionStage {
            kind: RotationDecisionStageKind::Eligibility,
            remaining: candidates
                .iter()
                .filter(|candidate| candidate.availability == RotationDecisionAvailability::Ready)
                .count(),
        }];
        let mut observe = |kind, remaining: &[ReadyCandidate]| {
            let ids = remaining
                .iter()
                .map(|candidate| candidate.id.as_str())
                .collect::<BTreeSet<_>>();
            for candidate in &mut candidates {
                if candidate.eliminated_at.is_none()
                    && !ids.contains(candidate.candidate_id.as_str())
                {
                    candidate.eliminated_at = Some(kind);
                }
            }
            stages.push(RotationDecisionStage {
                kind,
                remaining: remaining.len(),
            });
        };
        observe(RotationDecisionStageKind::PhysicalMember, &physical);
        observe(RotationDecisionStageKind::RecoveryBudget, &ready);
        let group = self.selection_group_observed(&ready, request, now_ms, &mut observe);
        let selected = self.choose_for_request(request, &group);
        let credit_limit =
            rotation_total_weight(&group).saturating_mul(ROTATION_CREDIT_HORIZON_ROUNDS);
        for candidate in &mut candidates {
            candidate.effective_weight = group
                .iter()
                .find(|member| member.id == candidate.candidate_id)
                .map(|member| member.weight);
            if matches!(
                self.mode,
                RotationMode::Automatic
                    | RotationMode::SubscriptionExpiry
                    | RotationMode::WeightedRoundRobin
            ) {
                candidate.rotation_score = candidate.effective_weight.map(|weight| {
                    let capacity = &self.candidates[&candidate.candidate_id]
                        .candidate
                        .capacity_key;
                    self.rotation_credit
                        .get(&(request.route_key.clone(), capacity.clone()))
                        .copied()
                        .unwrap_or_default()
                        .clamp(-credit_limit, credit_limit)
                        .saturating_add(i64::from(weight))
                });
            }
        }

        let mut fallback = request.clone();
        let mut rank = 1;
        // Bound repeated selection work on unusually large diagnostic pools.
        while let Some(selection) = (rank <= 256)
            .then(|| self.select(&fallback, now_ms))
            .flatten()
        {
            let capacity = &self.candidates[&selection.candidate_id]
                .candidate
                .capacity_key;
            if let Some(candidate) = candidates
                .iter_mut()
                .find(|candidate| candidate.candidate_id == selection.candidate_id)
            {
                candidate.fallback_rank = Some(rank);
            }
            for runtime in self
                .candidates
                .values()
                .filter(|runtime| runtime.candidate.capacity_key == *capacity)
            {
                fallback.tried.insert(runtime.candidate.id.clone());
            }
            rank += 1;
        }
        candidates.sort_by(|left, right| {
            left.fallback_rank
                .unwrap_or(usize::MAX)
                .cmp(&right.fallback_rank.unwrap_or(usize::MAX))
                .then_with(|| left.candidate_id.cmp(&right.candidate_id))
        });
        RotationDecisionTrace {
            observed_at_ms: now_ms,
            mode: self.mode,
            selected_candidate_id: selected.map(|candidate| candidate.id),
            owner_candidate_id: request.owner.clone(),
            fallback_order_complete: self.select(&fallback, now_ms).is_none(),
            stages,
            candidates,
        }
    }

    fn candidate_decision(
        &self,
        candidate: &RotationCandidate,
        request: &RotationRequest,
        now_ms: u64,
    ) -> RotationCandidateDecision {
        let (availability, block_reason, retry_at_ms) =
            match self.candidate_availability(request, &candidate.id, now_ms) {
                CandidateAvailability::Ready { .. } => {
                    (RotationDecisionAvailability::Ready, None, None)
                }
                CandidateAvailability::Busy(reason) => {
                    (RotationDecisionAvailability::Busy, Some(reason), None)
                }
                CandidateAvailability::WaitUntil { at_ms, reason } => (
                    RotationDecisionAvailability::Waiting,
                    Some(reason),
                    Some(at_ms),
                ),
                CandidateAvailability::Blocked(reason) => {
                    (RotationDecisionAvailability::Blocked, Some(reason), None)
                }
            };
        let quota_remaining = self.fresh_quota_remaining(&candidate.id, now_ms);
        let credits = self.fresh_provider_credits(&candidate.id, now_ms);
        let expiry = candidate.spend_deadline.and_then(|deadline| {
            Some(RotationExpiryEvidence {
                expires_at_ms: deadline.expires_at_ms,
                valid_until_ms: deadline.valid_until_ms,
                band: deadline.expiry_group(now_ms)?,
                reset_credits: deadline
                    .reset_credits
                    .filter(|resets| {
                        resets.observed_at_ms <= now_ms && now_ms <= resets.valid_until_ms
                    })
                    .map(|resets| resets.count),
                urgency_multiplier: quota_remaining
                    .map_or(1, |remaining| deadline.rotation_weight(remaining, now_ms)),
            })
        });
        RotationCandidateDecision {
            candidate_id: candidate.id.clone(),
            fallback_rank: None,
            availability,
            block_reason,
            configuration_block: None,
            retry_at_ms,
            eliminated_at: (availability != RotationDecisionAvailability::Ready)
                .then_some(RotationDecisionStageKind::Eligibility),
            priority: candidate.priority,
            configured_weight: candidate.weight,
            effective_weight: None,
            rotation_score: None,
            in_flight: self.capacity_in_flight(&candidate.capacity_key),
            effective_capacity: self
                .capacity_limits
                .get(&candidate.capacity_key)
                .copied()
                .filter(|limit| *limit > 0)
                .unwrap_or(self.max_in_flight)
                .min(self.max_in_flight),
            quota_remaining_basis_points: quota_remaining,
            provider_credits_micro_units: credits.filter(|credits| *credits != u64::MAX),
            provider_credits_unlimited: credits == Some(u64::MAX),
            expiry,
            preferred: request.preferred.as_deref() == Some(&candidate.id),
            warm_alternative: request.warm_fallbacks.contains(&candidate.id),
        }
    }
}
