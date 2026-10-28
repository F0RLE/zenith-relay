use super::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveModelRuntime {
    pub model: String,
    pub request_count: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRetryRuntime {
    pub model: String,
    pub retry_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateResourceHeadroom {
    pub quota_remaining_basis_points: Option<u64>,
    pub provider_credits_micro_units: Option<u64>,
    pub provider_credits_unlimited: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateRuntimeSnapshot {
    pub candidate_id: String,
    pub kind: CandidateKind,
    pub available: bool,
    /// All usable routes are waiting for quota or a provider rate limit.
    #[serde(default)]
    pub quota_wait: bool,
    /// Admission is waiting for a physical capacity or recovery lease.
    #[serde(default)]
    pub capacity_wait: bool,
    /// True only when fresh text routes agree on the same physical member.
    #[serde(default)]
    pub next_for_new_request: bool,
    /// Common fallback preference across fresh text routes, not future dispatches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection_rank: Option<usize>,
    /// Fresh resource evidence from admission, not a model-independent queue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_headroom: Option<CandidateResourceHeadroom>,
    #[serde(default)]
    pub activity_revision: u64,
    #[serde(default)]
    pub runtime_id: u64,
    pub in_flight: u32,
    #[serde(default)]
    pub active_request_count: u32,
    #[serde(default)]
    pub active_models: Vec<ActiveModelRuntime>,
    /// Model-scoped cooldowns that are still active. The aggregate retry
    /// deadline remains useful for global cooldowns.
    #[serde(default)]
    pub model_retries: Vec<ModelRetryRuntime>,
    pub last_used_at_ms: Option<u64>,
    pub next_retry_at_ms: Option<u64>,
    pub half_open: bool,
    pub dispatches: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReason {
    ResponseAffinity,
    PromptCacheAffinity,
    OnlyEligible,
    RoutingTier,
    SourceRole,
    ParallelLoad,
    SourceLoad,
    PoolPolicy,
    QuotaHeadroom,
    ProviderCredits,
    SubscriptionExpiry,
    SubscriptionPlan,
    ManualPriority,
    FairRotation,
    FallbackAttempt,
    StableTieBreak,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDiagnostics {
    pub reason: SelectionReason,
    pub eligible_candidates: u32,
    pub quota_remaining_basis_points: Option<u64>,
    pub in_flight_before: u32,
    pub dispatches_before: u64,
    /// Safe endpoint classification populated once the executor has resolved
    /// the actual upstream route.  It intentionally contains no host, query,
    /// credential, or provider response data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_kind: Option<String>,
    /// Upstream's declaration, not independently verified model identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstream_response_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<crate::usage::TransportDiagnostics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_context: Option<crate::usage::CacheContextDiagnostics>,
    /// Admission evidence only, not a provider cache-hit or cache-miss verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_affinity: Option<super::super::rotation::CacheAffinityDiagnostics>,
    /// The executor's actual disposition, never inferred from HTTP status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_outcome: Option<AttemptOutcome>,
    /// Logical execution timing; the event's latency/TTFT still describe its send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timing: Option<RequestTiming>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestTiming {
    pub duration_ms: u64,
    pub ttft_ms: Option<u64>,
    /// A request-loop finalization, not another physical attempt or quota observation.
    pub projection_only: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptOutcome {
    Completed,
    ToolDiscovery,
    RetrySelection,
    RetryRepair,
    RetryAuthorization,
    ReturnedFailure,
    ExecutionUncertain,
    ClientDisconnected,
}

impl PoolScheduler {
    pub fn runtime_order(&self, now_ms: u64) -> Vec<CandidateRuntimeSnapshot> {
        self.runtime_order_for(
            &CandidateScope::default(),
            &crate::ModelRules::default(),
            &WireApi::ALL_NATIVE,
            now_ms,
        )
    }

    pub(crate) fn runtime_order_for(
        &self,
        scope: &CandidateScope,
        models: &crate::ModelRules,
        protocols: &[WireApi],
        now_ms: u64,
    ) -> Vec<CandidateRuntimeSnapshot> {
        self.clone()
            .into_runtime_order(scope, models, protocols, now_ms)
    }

    /// Own a cloned scheduler so the live lock can be released before preview.
    /// Preview must not advance credits, circuits, or recovery on the scheduler
    /// that is serving requests.
    pub(crate) fn into_runtime_order(
        mut self,
        scope: &CandidateScope,
        models: &crate::ModelRules,
        protocols: &[WireApi],
        now_ms: u64,
    ) -> Vec<CandidateRuntimeSnapshot> {
        self.sync_all_rotation_candidates();
        let selection_order = self.preview_selection_order(scope, models, protocols, now_ms);
        let selection_ranks = selection_order
            .iter()
            .enumerate()
            .map(|(rank, id)| (members::member_key(&self.candidates[id]), rank))
            .collect::<BTreeMap<_, _>>();
        let mut snapshots = self
            .candidates
            .values()
            .map(|candidate| {
                let mut available = false;
                let mut quota_wait = false;
                let mut capacity_wait = false;
                let mut half_open = false;
                let mut retries = BTreeMap::<String, u64>::new();
                for model in &candidate.models {
                    if !models.allows(model)
                        || !self.rotation_visible(candidate, model, protocols, scope, now_ms)
                    {
                        continue;
                    }
                    let operation = Self::model_operation(model);
                    let candidate_request = self.rotation_request(
                        None,
                        Some(&candidate.id),
                        model,
                        operation,
                        BTreeSet::from([candidate.id.clone()]),
                    );
                    let circuit = self
                        .rotation
                        .circuit(&candidate.id, &candidate_request.route_key);
                    half_open |= circuit.state == super::super::rotation::CircuitState::HalfOpen;
                    match self.rotation.candidate_availability(
                        &candidate_request,
                        &candidate.id,
                        now_ms,
                    ) {
                        super::super::rotation::CandidateAvailability::Ready { .. } => {
                            let lane_ready = operation != RotationOperation::Image
                                || self.lane_allows(candidate, InFlightLane::Image);
                            available |= lane_ready;
                            capacity_wait |= !lane_ready;
                        }
                        super::super::rotation::CandidateAvailability::Busy(_) => {
                            capacity_wait = true;
                        }
                        super::super::rotation::CandidateAvailability::WaitUntil {
                            at_ms,
                            reason,
                        } => {
                            quota_wait |= reason
                                == super::super::rotation::CandidateBlockReason::QuotaExhausted
                                || (reason
                                    == super::super::rotation::CandidateBlockReason::RateLimited
                                    && self.cooldown_reason_for(candidate, model, now_ms)
                                        == CooldownReason::RateLimit);
                            retries
                                .entry(model.clone())
                                .and_modify(|at| *at = (*at).max(at_ms))
                                .or_insert(at_ms);
                        }
                        super::super::rotation::CandidateAvailability::Blocked(
                            super::super::rotation::CandidateBlockReason::QuotaExhausted,
                        ) => quota_wait = true,
                        _ => {}
                    }
                }
                let mut model_retries = retries
                    .into_iter()
                    .map(|(model, retry_at_ms)| ModelRetryRuntime { model, retry_at_ms })
                    .collect::<Vec<_>>();
                model_retries.sort_by_key(|retry| retry.retry_at_ms);
                let provider_credits = self.rotation.fresh_provider_credits(&candidate.id, now_ms);
                CandidateRuntimeSnapshot {
                    candidate_id: candidate.id.clone(),
                    kind: candidate.kind,
                    available,
                    quota_wait: quota_wait && !available,
                    capacity_wait: capacity_wait && !available,
                    next_for_new_request: selection_order.first() == Some(&candidate.id),
                    selection_rank: selection_ranks
                        .get(&members::member_key(candidate))
                        .copied(),
                    resource_headroom: Some(CandidateResourceHeadroom {
                        quota_remaining_basis_points: self
                            .rotation
                            .fresh_quota_remaining(&candidate.id, now_ms),
                        provider_credits_micro_units: provider_credits
                            .filter(|_| !candidate.provider_credits_unlimited),
                        provider_credits_unlimited: provider_credits.is_some()
                            && candidate.provider_credits_unlimited,
                    }),
                    activity_revision: 0,
                    runtime_id: 0,
                    in_flight: self.in_flight_count(&candidate.id, InFlightLane::Text),
                    active_request_count: self.active_request_count(&candidate.id),
                    active_models: self.active_models_for(&candidate.id),
                    next_retry_at_ms: model_retries.first().map(|retry| retry.retry_at_ms),
                    model_retries,
                    last_used_at_ms: candidate.last_used_at,
                    half_open,
                    dispatches: self.dispatch_count(&candidate.id, InFlightLane::Text),
                }
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| {
            (
                !snapshot.active_request_count.gt(&0),
                !snapshot.available,
                !snapshot.next_for_new_request,
                self.member_policy(&self.candidates[&snapshot.candidate_id])
                    .map_or(usize::MAX, |(rank, _)| rank),
                snapshot.candidate_id.clone(),
            )
        });
        snapshots
    }

    fn preview_selection_order(
        &mut self,
        scope: &CandidateScope,
        models: &crate::ModelRules,
        protocols: &[WireApi],
        now_ms: u64,
    ) -> Vec<String> {
        let routes = self
            .candidates
            .values()
            .flat_map(|candidate| {
                candidate
                    .models
                    .iter()
                    .filter(|model| {
                        !crate::runtime::is_image_model_id(model)
                            && models.allows(model)
                            && candidate.is_configured(model, protocols, scope)
                    })
                    .flat_map(|model| {
                        protocols
                            .iter()
                            .copied()
                            .filter(|protocol| {
                                if candidate.model_protocols.is_empty() {
                                    *protocol == candidate.protocol
                                } else {
                                    candidate.supports_protocol(model, &[*protocol])
                                }
                            })
                            .map(|protocol| (crate::model_id_key(model), protocol))
                    })
            })
            .collect::<BTreeSet<_>>();
        // The cloned snapshot is fixed: prepare admission once per route, but
        // only when reached. Divergent model routes often make the first rank
        // ambiguous, so do not prepare the rest of the catalog in that case.
        let mut requests = Vec::new();
        let mut order = Vec::new();
        loop {
            let mut selected_candidate_id: Option<String> = None;
            for (route_index, (model, protocol)) in routes.iter().enumerate() {
                if route_index == requests.len() {
                    let Some(mut request) = self.rotation_request_from_synced_state(
                        &SelectionRequest {
                            model,
                            allowed_protocols: &[*protocol],
                            scope,
                            tried: &HashSet::new(),
                            response_affinity_key: None,
                            prompt_affinity_key: None,
                            now_ms,
                        },
                        RotationOperation::Text,
                    ) else {
                        return order;
                    };
                    if let Some(allowed) = &mut request.allowed_candidates {
                        allowed.retain(|id| {
                            self.lane_allows(&self.candidates[id], InFlightLane::Text)
                        });
                    }
                    requests.push(request);
                }
                let request = &requests[route_index];
                let Some(selected) = self.rotation.select(request, now_ms) else {
                    return order;
                };
                if selected_candidate_id.as_ref().is_some_and(|previous| {
                    members::member_key(&self.candidates[previous])
                        != members::member_key(&self.candidates[&selected.candidate_id])
                }) {
                    return order;
                }
                selected_candidate_id.get_or_insert(selected.candidate_id);
            }
            let Some(selected) = selected_candidate_id else {
                return order;
            };
            let member = members::member_key(&self.candidates[&selected]);
            // Exclude every binding of the physical member, without reserving
            // requests or advancing the live rotation cursor/weighted credits.
            for request in &mut requests {
                if let Some(allowed) = &mut request.allowed_candidates {
                    allowed.retain(|id| members::member_key(&self.candidates[id]) != member);
                }
            }
            order.push(selected);
        }
    }
}

impl PoolScheduler {
    pub(super) fn diagnostics(
        &self,
        candidate_id: &str,
        reason: SelectionReason,
        eligible_candidates: usize,
        lane: InFlightLane,
    ) -> Option<RoutingDiagnostics> {
        let candidate = self.candidates.get(candidate_id)?;
        Some(RoutingDiagnostics {
            reason,
            eligible_candidates: u32::try_from(eligible_candidates).unwrap_or(u32::MAX),
            quota_remaining_basis_points: match candidate.quota {
                CandidateQuota::Available(remaining) => Some(remaining),
                CandidateQuota::Unknown
                | CandidateQuota::CreditFallback
                | CandidateQuota::Exhausted
                | CandidateQuota::Stale => None,
            },
            in_flight_before: self.in_flight_count(candidate_id, lane),
            dispatches_before: self.dispatch_count(candidate_id, lane),
            endpoint_kind: None,
            upstream_response_model: None,
            transport: None,
            cache_context: None,
            cache_affinity: None,
            attempt_outcome: None,
            request_timing: None,
        })
    }
}
