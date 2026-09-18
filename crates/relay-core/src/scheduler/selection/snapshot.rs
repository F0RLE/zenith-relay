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
        let next = self.preview_new_request_candidate(scope, models, protocols, now_ms);
        let mut candidates = self
            .candidates
            .values()
            .map(|candidate| {
                (
                    candidate,
                    self.is_runtime_available(candidate, now_ms),
                    self.in_flight_count(&candidate.id, InFlightLane::Text),
                )
            })
            .collect::<Vec<_>>();
        candidates.sort_by(
            |(left, left_available, left_in_flight), (right, right_available, right_in_flight)| {
                (right_in_flight > &0)
                    .cmp(&(left_in_flight > &0))
                    .then_with(|| right_available.cmp(left_available))
                    .then_with(|| self.compare_preference(right, left, InFlightLane::Text, now_ms))
            },
        );
        candidates
            .into_iter()
            .map(|(candidate, available, in_flight)| {
                let active_models = self.active_models_for(&candidate.id);
                let mut model_retries = candidate
                    .cooldowns
                    .iter()
                    .filter(|(model, retry_at_ms)| model.as_str() != "*" && **retry_at_ms > now_ms)
                    .map(|(model, retry_at_ms)| ModelRetryRuntime {
                        model: model.clone(),
                        retry_at_ms: *retry_at_ms,
                    })
                    .collect::<Vec<_>>();
                model_retries.sort_by_key(|retry| retry.retry_at_ms);
                CandidateRuntimeSnapshot {
                    candidate_id: candidate.id.clone(),
                    kind: candidate.kind,
                    available,
                    next_for_new_request: next.as_deref() == Some(candidate.id.as_str()),
                    activity_revision: 0,
                    runtime_id: 0,
                    in_flight,
                    active_request_count: self.active_request_count(&candidate.id),
                    active_models,
                    model_retries,
                    last_used_at_ms: candidate.last_used_at,
                    next_retry_at_ms: candidate
                        .cooldowns
                        .values()
                        .copied()
                        .filter(|retry_at_ms| *retry_at_ms > now_ms)
                        .min(),
                    half_open: self.reservations.is_probing(&candidate.id),
                    dispatches: self.dispatch_count(&candidate.id, InFlightLane::Text),
                }
            })
            .collect()
    }

    fn is_runtime_available(&self, candidate: &RuntimeCandidate, now_ms: u64) -> bool {
        let scope = CandidateScope::default();
        candidate
            .models
            .iter()
            .any(|model| self.is_eligible(candidate, model, &[candidate.protocol], &scope, now_ms))
    }

    fn preview_new_request_candidate(
        &self,
        scope: &CandidateScope,
        models: &crate::ModelRules,
        protocols: &[WireApi],
        now_ms: u64,
    ) -> Option<String> {
        let mut routes = BTreeMap::<(String, WireApi), Vec<&RuntimeCandidate>>::new();
        for candidate in self.candidates.values() {
            for model in &candidate.models {
                if models.allows(model) && candidate.is_configured(model, protocols, scope) {
                    routes
                        .entry((model.to_ascii_lowercase(), candidate.protocol))
                        .or_default()
                        .push(candidate);
                }
            }
        }
        let mut next: Option<&RuntimeCandidate> = None;
        for ((model, protocol), candidates) in routes {
            // Without a request's model or protocol a pool-wide text preview
            // is meaningful only when every applicable selection agrees.
            let eligible = candidates
                .iter()
                .copied()
                .filter(|candidate| {
                    self.lane_allows(candidate, InFlightLane::Text)
                        && self.is_eligible(candidate, &model, &[protocol], scope, now_ms)
                })
                .collect::<Vec<_>>();
            let (selected, _) = self.select_baseline(&eligible, InFlightLane::Text, now_ms)?;
            if next.is_some_and(|previous| {
                unified::member_key(previous) != unified::member_key(selected)
            }) {
                return None;
            }
            next.get_or_insert(selected);
        }
        next.map(|candidate| candidate.id.clone())
    }
}
