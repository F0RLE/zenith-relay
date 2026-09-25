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
pub struct CandidateRuntimeSnapshot {
    pub candidate_id: String,
    pub kind: CandidateKind,
    pub available: bool,
    /// True only when fresh text routes agree on the same physical member.
    #[serde(default)]
    pub next_for_new_request: bool,
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
}

impl PoolScheduler {
    pub fn runtime_order(&self, now_ms: u64) -> Vec<CandidateRuntimeSnapshot> {
        self.runtime_order_for(
            &CandidateScope::default(),
            &crate::ModelRules::default(),
            &WireApi::ALL,
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
        // Preview is a read-only projection: neither weighted credits nor
        // capacity, circuit epochs or recovery permits can advance here.
        let mut projection = self.clone();
        projection.sync_all_rotation_candidates();
        let next = projection.preview_new_request_candidate(scope, models, protocols, now_ms);
        let mut snapshots = projection
            .candidates
            .values()
            .map(|candidate| {
                let mut available = false;
                let mut half_open = false;
                let mut retries = BTreeMap::<String, u64>::new();
                for model in &candidate.models {
                    if !models.allows(model)
                        || !projection.rotation_visible(candidate, model, protocols, scope, now_ms)
                    {
                        continue;
                    }
                    let operation = Self::model_operation(model);
                    let request = projection.rotation_request(
                        None,
                        Some(&candidate.id),
                        model,
                        operation,
                        BTreeSet::from([candidate.id.clone()]),
                    );
                    let circuit = projection
                        .rotation
                        .circuit(&candidate.id, &request.route_key);
                    half_open |= circuit.state == super::super::rotation::CircuitState::HalfOpen;
                    match projection.rotation.candidate_availability(
                        &request,
                        &candidate.id,
                        now_ms,
                    ) {
                        super::super::rotation::CandidateAvailability::Ready { .. } => {
                            available |= operation != RotationOperation::Image
                                || projection.lane_allows(candidate, InFlightLane::Image);
                        }
                        super::super::rotation::CandidateAvailability::WaitUntil {
                            at_ms, ..
                        } => {
                            retries
                                .entry(model.clone())
                                .and_modify(|at| *at = (*at).max(at_ms))
                                .or_insert(at_ms);
                        }
                        _ => {}
                    }
                }
                let mut model_retries = retries
                    .into_iter()
                    .map(|(model, retry_at_ms)| ModelRetryRuntime { model, retry_at_ms })
                    .collect::<Vec<_>>();
                model_retries.sort_by_key(|retry| retry.retry_at_ms);
                CandidateRuntimeSnapshot {
                    candidate_id: candidate.id.clone(),
                    kind: candidate.kind,
                    available,
                    next_for_new_request: next.as_deref() == Some(candidate.id.as_str()),
                    activity_revision: 0,
                    runtime_id: 0,
                    in_flight: projection.in_flight_count(&candidate.id, InFlightLane::Text),
                    active_request_count: projection.active_request_count(&candidate.id),
                    active_models: projection.active_models_for(&candidate.id),
                    next_retry_at_ms: model_retries.first().map(|retry| retry.retry_at_ms),
                    model_retries,
                    last_used_at_ms: candidate.last_used_at,
                    half_open,
                    dispatches: projection.dispatch_count(&candidate.id, InFlightLane::Text),
                }
            })
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|entry| {
            (
                !entry.active_request_count.gt(&0),
                !entry.available,
                !entry.next_for_new_request,
                projection
                    .member_policy(&projection.candidates[&entry.candidate_id])
                    .map_or(usize::MAX, |(rank, _)| rank),
                entry.candidate_id.clone(),
            )
        });
        snapshots
    }

    fn preview_new_request_candidate(
        &mut self,
        scope: &CandidateScope,
        models: &crate::ModelRules,
        protocols: &[WireApi],
        now_ms: u64,
    ) -> Option<String> {
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
                    .map(|model| (model.to_ascii_lowercase(), candidate.protocol))
            })
            .collect::<BTreeSet<_>>();
        let mut next: Option<String> = None;
        for (model, protocol) in routes {
            let selected = self.select(SelectionRequest {
                model: &model,
                allowed_protocols: &[protocol],
                scope,
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms,
            })?;
            if next.as_ref().is_some_and(|previous| {
                members::member_key(&self.candidates[previous])
                    != members::member_key(&self.candidates[&selected.candidate_id])
            }) {
                return None;
            }
            next.get_or_insert(selected.candidate_id);
        }
        next
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
                CandidateQuota::Unknown | CandidateQuota::Exhausted | CandidateQuota::Stale => None,
            },
            in_flight_before: self.in_flight_count(candidate_id, lane),
            dispatches_before: self.dispatch_count(candidate_id, lane),
            endpoint_kind: None,
        })
    }
}
