mod routing_policy;
mod snapshot;

use super::activity::{InFlightLane, SchedulerActivity};
use super::affinity::AffinityCache;
use super::candidate::{CandidateHealth, CandidateKind, CandidateScope, RuntimeCandidate};
use super::capacity::{CandidateQuota, QUOTA_STALE_AFTER_MS};
use super::cooldown::{has_expired_cooldown, CooldownReason};
use crate::WireApi;
pub use routing_policy::RoutingStrategy;
use routing_policy::{candidate_kind_preference, routing_tier};
#[cfg(test)]
use routing_policy::{API_SOURCE_PRIMARY_PRIORITY, API_SOURCE_RESERVE_PRIORITY};
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

pub use snapshot::{
    ActiveModelRuntime, CandidateRuntimeSnapshot, ModelRetryRuntime, RoutingDiagnostics,
    SelectionReason,
};

// Keep response ownership across busy personal pools without allowing an
// unbounded in-memory map. The durable store uses the same capacity policy.
const RESPONSE_AFFINITY_MAX_ENTRIES: usize = 16_384;
pub const RESPONSE_AFFINITY_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
const PROMPT_AFFINITY_MAX_ENTRIES: usize = 16_384;
pub const PROMPT_AFFINITY_TTL_MS: u64 = 60 * 60 * 1_000;
const PROMPT_AFFINITY_MAX_IN_FLIGHT_SKEW: u32 = 1;
const PROMPT_AFFINITY_QUOTA_SLACK_BPS: u64 = 500;
const MAX_OAUTH_IMAGE_IN_FLIGHT: u32 = 1;
const PROVIDER_STORM_WINDOW_MS: u64 = 10_000;
const PROVIDER_STORM_OPEN_MS: u64 = 30_000;
const PROVIDER_STORM_THRESHOLD: usize = 3;

pub struct SelectionRequest<'a> {
    pub model: &'a str,
    pub allowed_protocols: &'a [WireApi],
    pub scope: &'a CandidateScope,
    pub tried: &'a HashSet<String>,
    pub response_affinity_key: Option<&'a str>,
    pub prompt_affinity_key: Option<&'a str>,
    pub now_ms: u64,
}

#[derive(Clone, Copy)]
pub(crate) struct CooldownRequest<'a> {
    pub(crate) scope: &'a str,
    pub(crate) policy_model: &'a str,
    pub(crate) allowed_protocols: &'a [WireApi],
    pub(crate) request_scope: &'a CandidateScope,
    pub(crate) retry_at_ms: u64,
    pub(crate) reason: CooldownReason,
    pub(crate) now_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    pub candidate_id: String,
    pub response_affinity_hit: bool,
    pub half_open_probe: bool,
    pub diagnostics: RoutingDiagnostics,
}

#[derive(Clone, Debug)]
pub struct PoolScheduler {
    candidates: BTreeMap<String, RuntimeCandidate>,
    cooldown_reasons: BTreeMap<(String, String), CooldownReason>,
    response_affinity: AffinityCache,
    prompt_affinity: AffinityCache,
    activity: SchedulerActivity,
    half_open: BTreeSet<(String, String)>,
    routing_strategy: RoutingStrategy,
    quota_stale_after_ms: u64,
    subscription_expires_at_ms: BTreeMap<String, u64>,
    subscription_plans: BTreeMap<String, String>,
    subscription_plan_ranks: BTreeMap<String, usize>,
    protected_candidate: Option<(String, u64)>,
    execution_fences: BTreeMap<String, u32>,
    capability_blocks: BTreeSet<(String, String)>,
    provider_storm_breakers: BTreeMap<(String, String), StormBreaker>,
    provider_storm_breaker_enabled: bool,
    cooldown_after_failures: u32,
    keep_last_candidate_available: bool,
}

#[derive(Clone, Debug, Default)]
struct StormBreaker {
    failures: VecDeque<u64>,
    open_until_ms: u64,
}

impl Default for PoolScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl PoolScheduler {
    pub fn new() -> Self {
        Self {
            candidates: BTreeMap::new(),
            cooldown_reasons: BTreeMap::new(),
            response_affinity: AffinityCache::new(
                RESPONSE_AFFINITY_MAX_ENTRIES,
                RESPONSE_AFFINITY_TTL_MS,
            ),
            prompt_affinity: AffinityCache::new(
                PROMPT_AFFINITY_MAX_ENTRIES,
                PROMPT_AFFINITY_TTL_MS,
            ),
            activity: SchedulerActivity::default(),
            half_open: BTreeSet::new(),
            routing_strategy: RoutingStrategy::Adaptive,
            quota_stale_after_ms: QUOTA_STALE_AFTER_MS,
            subscription_expires_at_ms: BTreeMap::new(),
            subscription_plans: BTreeMap::new(),
            subscription_plan_ranks: BTreeMap::new(),
            protected_candidate: None,
            execution_fences: BTreeMap::new(),
            capability_blocks: BTreeSet::new(),
            provider_storm_breakers: BTreeMap::new(),
            provider_storm_breaker_enabled: false,
            // Direct scheduler users historically applied cooldowns
            // immediately. GatewayRuntime installs the user-facing policy.
            cooldown_after_failures: 1,
            keep_last_candidate_available: false,
        }
    }

    pub(crate) fn set_cooldown_policy(
        &mut self,
        cooldown_after_failures: u8,
        keep_last_candidate_available: bool,
    ) {
        self.cooldown_after_failures = u32::from(cooldown_after_failures.max(1));
        self.keep_last_candidate_available = keep_last_candidate_available;
    }

    pub fn set_provider_storm_breaker_enabled(&mut self, enabled: bool) {
        self.provider_storm_breaker_enabled = enabled;
        if !enabled {
            self.provider_storm_breakers.clear();
        }
    }

    pub fn set_routing_strategy(&mut self, strategy: RoutingStrategy) {
        if self.routing_strategy != strategy {
            self.routing_strategy = strategy;
            self.activity.clear_dispatches();
        }
    }

    pub fn set_quota_stale_after_ms(&mut self, stale_after_ms: u64) {
        self.quota_stale_after_ms = stale_after_ms.max(1);
    }

    pub fn set_candidate_subscription_expiry(
        &mut self,
        candidate_id: &str,
        expires_at_ms: Option<u64>,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        match expires_at_ms {
            Some(expires_at_ms) => {
                self.subscription_expires_at_ms
                    .insert(candidate_id.to_string(), expires_at_ms);
            }
            None => {
                self.subscription_expires_at_ms.remove(candidate_id);
            }
        }
        true
    }

    pub fn set_subscription_plan_order(&mut self, order: &[String]) {
        self.subscription_plan_ranks = order
            .iter()
            .enumerate()
            .map(|(rank, plan)| (plan.clone(), rank))
            .collect();
    }

    pub fn set_candidate_subscription_plan(
        &mut self,
        candidate_id: &str,
        plan_type: Option<&str>,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.subscription_plans
            .insert(candidate_id.to_string(), subscription_plan_key(plan_type));
        true
    }

    pub fn upsert(&mut self, candidate: RuntimeCandidate) {
        let candidate_id = candidate.id.clone();
        self.cooldown_reasons.retain(|(id, model), _| {
            id != &candidate_id || candidate.cooldowns.contains_key(model)
        });
        self.candidates.insert(candidate.id.clone(), candidate);
    }

    pub fn remove(&mut self, candidate_id: &str) -> Option<RuntimeCandidate> {
        self.response_affinity.invalidate_candidate(candidate_id);
        self.prompt_affinity.invalidate_candidate(candidate_id);
        self.activity.remove_candidate(candidate_id);
        self.half_open
            .retain(|(half_open_candidate, _)| half_open_candidate != candidate_id);
        self.subscription_expires_at_ms.remove(candidate_id);
        self.subscription_plans.remove(candidate_id);
        self.execution_fences.remove(candidate_id);
        self.capability_blocks
            .retain(|(blocked_candidate, _)| blocked_candidate != candidate_id);
        self.cooldown_reasons
            .retain(|(cooled_candidate, _), _| cooled_candidate != candidate_id);
        let source_id = self
            .candidates
            .get(candidate_id)
            .map(|candidate| candidate.source_id.clone());
        if let Some(source_id) = source_id {
            self.provider_storm_breakers
                .retain(|(source, _), _| source != &source_id);
        }
        if self
            .protected_candidate
            .as_ref()
            .is_some_and(|(protected_id, _)| protected_id == candidate_id)
        {
            self.protected_candidate = None;
        }
        self.candidates.remove(candidate_id)
    }

    pub fn candidate(&self, candidate_id: &str) -> Option<&RuntimeCandidate> {
        self.candidates.get(candidate_id)
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

    pub fn runtime_order(&self, now_ms: u64) -> Vec<CandidateRuntimeSnapshot> {
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
                    .then_with(|| self.compare_preference(right, left, InFlightLane::Text))
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
                    half_open: self
                        .half_open
                        .iter()
                        .any(|(candidate_id, _)| candidate_id == &candidate.id),
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
        if quota_changed {
            self.activity.clear_dispatches();
        }
        true
    }

    pub fn update_candidate_quota_at(
        &mut self,
        candidate_id: &str,
        quota: CandidateQuota,
        quota_updated_at_ms: Option<u64>,
        quota_reset_at_ms: Option<u64>,
    ) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        let changed = candidate.quota != quota
            || candidate.quota_updated_at_ms != quota_updated_at_ms
            || candidate.quota_reset_at_ms != quota_reset_at_ms;
        candidate.quota = quota;
        candidate.quota_updated_at_ms = quota_updated_at_ms;
        candidate.quota_reset_at_ms = quota_reset_at_ms;
        if changed {
            self.activity.clear_dispatches();
        }
        true
    }

    pub fn set_execution_fence(&mut self, candidate_id: &str, fenced: bool) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        if fenced {
            let count = self
                .execution_fences
                .entry(candidate_id.to_string())
                .or_default();
            *count = count.saturating_add(1);
        } else if let Some(count) = self.execution_fences.get_mut(candidate_id) {
            if *count <= 1 {
                self.execution_fences.remove(candidate_id);
            } else {
                *count -= 1;
            }
        }
        true
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

    pub fn record_provider_rate_limit(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> bool {
        if !self.provider_storm_breaker_enabled {
            return false;
        }
        let Some(candidate) = self
            .candidates
            .get(candidate_id)
            .filter(|candidate| candidate.kind == CandidateKind::ApiSource)
        else {
            return false;
        };
        let breaker = self
            .provider_storm_breakers
            .entry((candidate.source_id.clone(), model.to_ascii_lowercase()))
            .or_default();
        while breaker
            .failures
            .front()
            .is_some_and(|observed| now_ms.saturating_sub(*observed) > PROVIDER_STORM_WINDOW_MS)
        {
            breaker.failures.pop_front();
        }
        breaker.failures.push_back(now_ms);
        if breaker.failures.len() >= PROVIDER_STORM_THRESHOLD {
            breaker.open_until_ms = now_ms.saturating_add(PROVIDER_STORM_OPEN_MS);
        }
        breaker.open_until_ms > now_ms
    }

    pub fn set_candidate_health(&mut self, candidate_id: &str, health: CandidateHealth) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.health = health;
        true
    }

    pub fn select(&mut self, request: SelectionRequest<'_>) -> Option<Selection> {
        self.select_for(request, InFlightLane::Text)
    }

    pub(crate) fn select_image(&mut self, request: SelectionRequest<'_>) -> Option<Selection> {
        self.select_for(request, InFlightLane::Image)
    }

    fn select_for(
        &mut self,
        request: SelectionRequest<'_>,
        lane: InFlightLane,
    ) -> Option<Selection> {
        if let Some(key) = request.response_affinity_key {
            if let Some(candidate_id) = self
                .response_affinity
                .get(key, request.now_ms)
                .map(str::to_string)
            {
                let eligible = self.candidates.get(&candidate_id).is_some_and(|candidate| {
                    !request.tried.contains(&candidate_id)
                        && self.lane_allows(candidate, lane)
                        && self.is_eligible(
                            candidate,
                            request.model,
                            request.allowed_protocols,
                            request.scope,
                            request.now_ms,
                        )
                });
                if eligible {
                    self.response_affinity.refresh(key, request.now_ms);
                    let diagnostics = self.diagnostics(
                        &candidate_id,
                        SelectionReason::ResponseAffinity,
                        self.eligible_count(&request, lane),
                        lane,
                    )?;
                    return Some(Selection {
                        half_open_probe: self.is_half_open_probe(
                            &candidate_id,
                            request.model,
                            request.now_ms,
                        ),
                        candidate_id,
                        response_affinity_hit: true,
                        diagnostics,
                    });
                }
                return None;
            }
        }

        let prompt_affinity_candidate = request.prompt_affinity_key.and_then(|key| {
            self.prompt_affinity
                .get(key, request.now_ms)
                .map(str::to_string)
        });
        let explicit_prompt_cache_key = request
            .prompt_affinity_key
            .is_some_and(|key| key.starts_with("cache:"));

        let eligible = self
            .candidates
            .values()
            .filter(|candidate| {
                !request.tried.contains(&candidate.id)
                    && self.lane_allows(candidate, lane)
                    && self.is_eligible(
                        candidate,
                        request.model,
                        request.allowed_protocols,
                        request.scope,
                        request.now_ms,
                    )
            })
            .collect::<Vec<_>>();
        let baseline = eligible
            .iter()
            .copied()
            .max_by(|left, right| self.compare_preference(left, right, lane))?;
        let selected = prompt_affinity_candidate
            .as_deref()
            .and_then(|candidate_id| {
                eligible
                    .iter()
                    .copied()
                    .find(|candidate| candidate.id == candidate_id)
            })
            .filter(|preferred| {
                preferred.id != baseline.id
                    && self.prompt_affinity_allows(
                        preferred,
                        baseline,
                        lane,
                        explicit_prompt_cache_key,
                    )
            })
            .unwrap_or(baseline);
        let prompt_affinity_hit = selected.id != baseline.id;
        let runner_up = eligible
            .iter()
            .copied()
            .filter(|candidate| candidate.id != selected.id)
            .max_by(|left, right| self.compare_preference(left, right, lane));
        let reason = if prompt_affinity_hit {
            SelectionReason::PromptCacheAffinity
        } else if !request.tried.is_empty() {
            SelectionReason::FallbackAttempt
        } else {
            runner_up.map_or(SelectionReason::OnlyEligible, |runner_up| {
                self.selection_reason(selected, runner_up, lane)
            })
        };
        Some(Selection {
            candidate_id: selected.id.clone(),
            response_affinity_hit: false,
            half_open_probe: self.is_half_open_probe(&selected.id, request.model, request.now_ms),
            diagnostics: self.diagnostics(&selected.id, reason, eligible.len(), lane)?,
        })
    }

    fn lane_allows(&self, candidate: &RuntimeCandidate, lane: InFlightLane) -> bool {
        if candidate.kind != CandidateKind::OAuthAccount || lane == InFlightLane::Text {
            return true;
        }
        self.in_flight_count(&candidate.id, InFlightLane::Image) < MAX_OAUTH_IMAGE_IN_FLIGHT
    }

    fn is_half_open_probe(&self, candidate_id: &str, model: &str, now_ms: u64) -> bool {
        self.candidates
            .get(candidate_id)
            .is_some_and(|candidate| half_open_scope(candidate, model, now_ms).is_some())
    }

    fn diagnostics(
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

    fn eligible_count(&self, request: &SelectionRequest<'_>, lane: InFlightLane) -> usize {
        self.candidates
            .values()
            .filter(|candidate| {
                !request.tried.contains(&candidate.id)
                    && self.lane_allows(candidate, lane)
                    && self.is_eligible(
                        candidate,
                        request.model,
                        request.allowed_protocols,
                        request.scope,
                        request.now_ms,
                    )
            })
            .count()
    }

    pub fn earliest_retry_at(&mut self, request: SelectionRequest<'_>) -> Option<u64> {
        if let Some(key) = request.response_affinity_key {
            let candidate_id = self
                .response_affinity
                .get(key, request.now_ms)
                .map(str::to_string)?;
            if request.tried.contains(&candidate_id) {
                return None;
            }
            let candidate = self.candidates.get(&candidate_id)?;
            return self
                .quota_reserve_allows(candidate, request.now_ms)
                .then(|| {
                    candidate.retry_at_if_configured(
                        request.model,
                        request.allowed_protocols,
                        request.scope,
                        request.now_ms,
                    )
                })
                .flatten();
        }
        self.candidates
            .values()
            .filter(|candidate| !request.tried.contains(&candidate.id))
            .filter(|candidate| self.quota_reserve_allows(candidate, request.now_ms))
            .filter_map(|candidate| {
                candidate.retry_at_if_configured(
                    request.model,
                    request.allowed_protocols,
                    request.scope,
                    request.now_ms,
                )
            })
            .min()
    }

    pub(crate) fn all_applicable_cooldown(
        &mut self,
        request: SelectionRequest<'_>,
    ) -> Option<(u64, CooldownReason)> {
        if let Some(key) = request.response_affinity_key {
            let candidate_id = self
                .response_affinity
                .get(key, request.now_ms)
                .map(str::to_string)?;
            if request.tried.contains(&candidate_id) {
                return None;
            }
            let candidate = self.candidates.get(&candidate_id)?;
            if !self.quota_reserve_allows(candidate, request.now_ms) {
                return None;
            }
            let retry_at = candidate.retry_at_if_configured(
                request.model,
                request.allowed_protocols,
                request.scope,
                request.now_ms,
            )?;
            return Some((
                retry_at,
                self.cooldown_reason_for(candidate, request.model, request.now_ms),
            ));
        }

        let mut retry_at: Option<u64> = None;
        let mut reason = CooldownReason::RateLimit;
        for candidate in self.candidates.values() {
            if request.tried.contains(&candidate.id)
                || !self.quota_reserve_allows(candidate, request.now_ms)
                || !candidate.is_catalog_visible(
                    request.model,
                    request.allowed_protocols,
                    request.scope,
                )
            {
                continue;
            }
            let Some(candidate_retry_at) = candidate.retry_at_if_configured(
                request.model,
                request.allowed_protocols,
                request.scope,
                request.now_ms,
            ) else {
                // A visible candidate without an active cooldown is still
                // available after the bounded retry set.
                return None;
            };
            retry_at = Some(retry_at.map_or(candidate_retry_at, |current| {
                current.min(candidate_retry_at)
            }));
            reason = Self::aggregate_cooldown_reason(
                reason,
                self.cooldown_reason_for(candidate, request.model, request.now_ms),
            );
        }
        retry_at.map(|retry_at| (retry_at, reason))
    }

    fn cooldown_reason_for(
        &self,
        candidate: &RuntimeCandidate,
        model: &str,
        now_ms: u64,
    ) -> CooldownReason {
        let mut reason = None;
        for (candidate_model, retry_at) in &candidate.cooldowns {
            if *retry_at <= now_ms
                || (candidate_model.as_str() != "*" && !candidate_model.eq_ignore_ascii_case(model))
            {
                continue;
            }
            let candidate_reason = self
                .cooldown_reasons
                .get(&(candidate.id.clone(), candidate_model.clone()))
                .copied()
                .unwrap_or(CooldownReason::Transient);
            reason = Some(Self::aggregate_cooldown_reason(
                reason.unwrap_or(CooldownReason::RateLimit),
                candidate_reason,
            ));
        }
        reason.unwrap_or(CooldownReason::Transient)
    }

    fn aggregate_cooldown_reason(current: CooldownReason, next: CooldownReason) -> CooldownReason {
        match (current, next) {
            (CooldownReason::Mandatory, _) | (_, CooldownReason::Mandatory) => {
                CooldownReason::Mandatory
            }
            (CooldownReason::Transient, _) | (_, CooldownReason::Transient) => {
                CooldownReason::Transient
            }
            _ => CooldownReason::RateLimit,
        }
    }

    pub(crate) fn is_eligible(
        &self,
        candidate: &RuntimeCandidate,
        model: &str,
        allowed_protocols: &[WireApi],
        scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        !self.execution_fences.contains_key(&candidate.id)
            && !self
                .capability_blocks
                .contains(&(candidate.id.clone(), model.to_ascii_lowercase()))
            && !self.provider_storm_open(candidate, model, now_ms)
            && self.quota_reserve_allows(candidate, now_ms)
            && candidate.is_eligible(model, allowed_protocols, scope, now_ms)
            && half_open_scope(candidate, model, now_ms)
                .is_none_or(|scope| !self.half_open.contains(&(candidate.id.clone(), scope)))
    }

    pub fn bind_response_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.response_affinity.bind(key, candidate_id, now_ms);
        true
    }

    pub fn bind_prompt_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.prompt_affinity.bind(key, candidate_id, now_ms);
        true
    }

    /// Persist prompt affinity without allowing a temporary spillover
    /// candidate to become the new durable owner.  The existing owner is
    /// refreshed when the same candidate completes successfully and can only
    /// be replaced after it has been invalidated by the failure path.
    pub fn bind_prompt_affinity_sticky(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.prompt_affinity
            .bind_if_unbound_or_same(key, candidate_id, now_ms)
    }

    pub fn restore_response_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) || expires_at_ms <= now_ms {
            return false;
        }
        self.response_affinity
            .restore(key, candidate_id, expires_at_ms, now_ms);
        true
    }

    pub fn restore_prompt_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        expires_at_ms: u64,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) || expires_at_ms <= now_ms {
            return false;
        }
        self.prompt_affinity
            .restore(key, candidate_id, expires_at_ms, now_ms);
        true
    }

    pub fn has_response_affinity(&mut self, key: &str, now_ms: u64) -> bool {
        self.response_affinity.contains(key, now_ms)
    }

    pub fn has_prompt_affinity(&mut self, key: &str, now_ms: u64) -> bool {
        self.prompt_affinity.contains(key, now_ms)
    }

    pub fn invalidate_response_affinity(&mut self, key: &str) -> bool {
        self.response_affinity.invalidate(key)
    }

    #[cfg(test)]
    pub(crate) fn reserve(&mut self, candidate_id: &str) -> bool {
        self.reserve_for(candidate_id, "", 0)
    }

    pub(crate) fn reserve_for(&mut self, candidate_id: &str, model: &str, now_ms: u64) -> bool {
        self.reserve_for_lane(candidate_id, model, now_ms, InFlightLane::Text)
    }

    pub(crate) fn reserve_image_for(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
    ) -> bool {
        self.reserve_for_lane(candidate_id, model, now_ms, InFlightLane::Image)
    }

    fn reserve_for_lane(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
        lane: InFlightLane,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        if self
            .candidates
            .get(candidate_id)
            .is_some_and(|candidate| !self.lane_allows(candidate, lane))
        {
            return false;
        }
        if !model.is_empty() {
            let half_open_key = self
                .candidates
                .get(candidate_id)
                .and_then(|candidate| half_open_scope(candidate, model, now_ms))
                .map(|scope| (candidate_id.to_string(), scope));
            if half_open_key.is_some_and(|key| !self.half_open.insert(key)) {
                return false;
            }
        }
        self.activity.reserve(candidate_id, model, lane);
        true
    }

    #[cfg(test)]
    pub(crate) fn release(&mut self, candidate_id: &str) -> bool {
        self.release_for(candidate_id, None)
    }

    pub(crate) fn release_for(&mut self, candidate_id: &str, model: Option<&str>) -> bool {
        self.release_for_lane(candidate_id, model, InFlightLane::Text)
    }

    pub(crate) fn release_image_for(&mut self, candidate_id: &str, model: Option<&str>) -> bool {
        self.release_for_lane(candidate_id, model, InFlightLane::Image)
    }

    fn release_for_lane(
        &mut self,
        candidate_id: &str,
        model: Option<&str>,
        lane: InFlightLane,
    ) -> bool {
        if let Some(model) = model {
            self.half_open
                .remove(&(candidate_id.to_string(), model.to_ascii_lowercase()));
            self.half_open
                .remove(&(candidate_id.to_string(), "*".to_string()));
        } else {
            self.half_open
                .retain(|(half_open_candidate, _)| half_open_candidate != candidate_id);
        }
        self.activity.release(candidate_id, model, lane)
    }

    pub fn record_success(&mut self, candidate_id: &str, model: &str, now_ms: u64) -> bool {
        self.record_success_with_metrics(candidate_id, model, now_ms, None, 0)
    }

    pub fn record_success_with_metrics(
        &mut self,
        candidate_id: &str,
        model: &str,
        now_ms: u64,
        _output_tokens: Option<u64>,
        _latency_ms: u64,
    ) -> bool {
        let (provider_key, recovered) = {
            let Some(candidate) = self.candidates.get_mut(candidate_id) else {
                return false;
            };
            let provider_key = (candidate.source_id.clone(), model.to_ascii_lowercase());
            self.half_open
                .remove(&(candidate_id.to_string(), model.to_ascii_lowercase()));
            candidate.cooldowns.retain(|candidate_model, retry_at_ms| {
                let applies = candidate_model == "*" || candidate_model.eq_ignore_ascii_case(model);
                !applies || *retry_at_ms > now_ms
            });
            candidate.last_used_at = Some(now_ms);
            let recovered = !candidate
                .cooldowns
                .values()
                .any(|retry_at_ms| *retry_at_ms > now_ms);
            if recovered {
                candidate.health = CandidateHealth::Healthy;
                candidate.consecutive_failures = 0;
            }
            (provider_key, recovered)
        };
        if let Some(candidate) = self.candidates.get(candidate_id) {
            self.cooldown_reasons
                .retain(|(cooled_candidate, scope), _| {
                    cooled_candidate != candidate_id || candidate.cooldowns.contains_key(scope)
                });
        }
        self.provider_storm_breakers.remove(&provider_key);
        recovered
    }

    pub fn record_failure(&mut self, candidate_id: &str) -> Option<u32> {
        let candidate = self.candidates.get_mut(candidate_id)?;
        candidate.consecutive_failures = candidate.consecutive_failures.saturating_add(1);
        Some(candidate.consecutive_failures)
    }

    pub fn reset_failures(&mut self, candidate_id: &str) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.consecutive_failures = 0;
        true
    }

    pub fn set_cooldown(&mut self, candidate_id: &str, model: &str, retry_at_ms: u64) -> bool {
        self.set_cooldown_with_reason(candidate_id, model, retry_at_ms, CooldownReason::Transient)
    }

    pub(crate) fn set_cooldown_with_reason(
        &mut self,
        candidate_id: &str,
        model: &str,
        retry_at_ms: u64,
        reason: CooldownReason,
    ) -> bool {
        self.set_cooldown_with_reason_inner(
            candidate_id,
            CooldownRequest {
                scope: model,
                policy_model: model,
                allowed_protocols: &[],
                request_scope: &CandidateScope::default(),
                retry_at_ms,
                reason,
                now_ms: 0,
            },
            false,
        )
    }

    pub(crate) fn set_cooldown_with_reason_for_model_at(
        &mut self,
        candidate_id: &str,
        request: CooldownRequest<'_>,
    ) -> bool {
        self.set_cooldown_with_reason_inner(candidate_id, request, true)
    }

    fn set_cooldown_with_reason_inner(
        &mut self,
        candidate_id: &str,
        request: CooldownRequest<'_>,
        enforce_policy: bool,
    ) -> bool {
        if enforce_policy
            && request.reason == CooldownReason::Transient
            && !self.transient_cooldown_allowed(
                candidate_id,
                request.policy_model,
                request.allowed_protocols,
                request.request_scope,
                request.now_ms,
            )
        {
            return false;
        }
        let scope = if request.scope == "*" {
            "*".to_string()
        } else {
            request.scope.to_ascii_lowercase()
        };
        let previous = self
            .candidates
            .get(candidate_id)
            .and_then(|candidate| candidate.cooldowns.get(&scope).copied());
        let previous_reason = self
            .cooldown_reasons
            .get(&(candidate_id.to_string(), scope.clone()))
            .copied();
        let should_store_reason = previous.is_none_or(|current| {
            request.retry_at_ms > current
                || (request.retry_at_ms == current
                    && request.reason == CooldownReason::RateLimit
                    && previous_reason != Some(CooldownReason::Mandatory))
                || (request.reason == CooldownReason::Mandatory
                    && previous_reason != Some(CooldownReason::Mandatory))
        });
        {
            let Some(candidate) = self.candidates.get_mut(candidate_id) else {
                return false;
            };
            candidate
                .cooldowns
                .entry(scope.clone())
                .and_modify(|current| *current = (*current).max(request.retry_at_ms))
                .or_insert(request.retry_at_ms);
        }
        if should_store_reason {
            self.cooldown_reasons
                .insert((candidate_id.to_string(), scope.clone()), request.reason);
        }
        if scope == "*" {
            self.half_open
                .retain(|(half_open_candidate, _)| half_open_candidate != candidate_id);
        } else {
            self.half_open.remove(&(candidate_id.to_string(), scope));
        }
        true
    }

    fn transient_cooldown_allowed(
        &self,
        candidate_id: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        request_scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        let Some(candidate) = self.candidates.get(candidate_id) else {
            return false;
        };
        if candidate.consecutive_failures < self.cooldown_after_failures {
            return false;
        }
        if !self.keep_last_candidate_available {
            return true;
        }
        !self.is_last_applicable_candidate(
            candidate_id,
            model,
            allowed_protocols,
            request_scope,
            now_ms,
        )
    }

    fn is_last_applicable_candidate(
        &self,
        candidate_id: &str,
        model: &str,
        allowed_protocols: &[WireApi],
        request_scope: &CandidateScope,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.candidates
            .values()
            .filter(|candidate| {
                self.is_eligible(candidate, model, allowed_protocols, request_scope, now_ms)
            })
            .count()
            <= 1
    }

    pub fn clear_cooldown(&mut self, candidate_id: &str, model: &str) -> bool {
        let removed = self
            .candidates
            .get_mut(candidate_id)
            .map(|candidate| {
                let previous_len = candidate.cooldowns.len();
                candidate
                    .cooldowns
                    .retain(|candidate_model, _| !candidate_model.eq_ignore_ascii_case(model));
                candidate.cooldowns.len() != previous_len
            })
            .unwrap_or(false);
        if removed {
            self.cooldown_reasons
                .retain(|(cooled_candidate, scope), _| {
                    cooled_candidate != candidate_id || !scope.eq_ignore_ascii_case(model)
                });
        }
        removed
    }

    fn prompt_affinity_allows(
        &self,
        preferred: &RuntimeCandidate,
        baseline: &RuntimeCandidate,
        lane: InFlightLane,
        explicit_prompt_cache_key: bool,
    ) -> bool {
        let preferred_in_flight = self.in_flight_count(&preferred.id, lane);
        let baseline_in_flight = self.in_flight_count(&baseline.id, lane);
        if routing_tier(preferred) != routing_tier(baseline)
            || candidate_kind_preference(preferred) != candidate_kind_preference(baseline)
            || preferred_in_flight
                > baseline_in_flight.saturating_add(PROMPT_AFFINITY_MAX_IN_FLIGHT_SKEW)
        {
            return false;
        }
        // An explicit API source order is a hard route policy. Prompt affinity
        // may preserve a cache only when it does not demote that policy.
        if preferred.kind == CandidateKind::ApiSource
            && baseline.kind == CandidateKind::ApiSource
            && preferred.priority < baseline.priority
        {
            return false;
        }
        if explicit_prompt_cache_key {
            // An explicit provider cache key is a strong cache contract. Keep
            // its owner across quota differences while it remains eligible;
            // hard exhaustion and protected reserves were filtered above.
            return true;
        }
        // A derived session fingerprint is only a best-effort hint. Preserve
        // the regular quota-aware rotation for clients that do not send a
        // provider cache key.
        match (self.routing_quota(preferred), self.routing_quota(baseline)) {
            (CandidateQuota::Available(preferred), CandidateQuota::Available(baseline)) => {
                preferred.saturating_add(PROMPT_AFFINITY_QUOTA_SLACK_BPS) >= baseline
            }
            (CandidateQuota::Available(_), CandidateQuota::Unknown)
            | (CandidateQuota::Unknown, CandidateQuota::Unknown) => true,
            _ => false,
        }
    }

    fn provider_storm_open(&self, candidate: &RuntimeCandidate, model: &str, now_ms: u64) -> bool {
        self.provider_storm_breaker_enabled
            && self
                .provider_storm_breakers
                .get(&(candidate.source_id.clone(), model.to_ascii_lowercase()))
                .is_some_and(|breaker| breaker.open_until_ms > now_ms)
    }

    fn in_flight_count(&self, candidate_id: &str, lane: InFlightLane) -> u32 {
        self.activity.in_flight_count(candidate_id, lane)
    }

    fn active_request_count(&self, candidate_id: &str) -> u32 {
        self.activity.active_request_count(candidate_id)
    }

    fn active_models_for(&self, candidate_id: &str) -> Vec<ActiveModelRuntime> {
        self.activity
            .active_models_for(candidate_id)
            .into_iter()
            .map(|(model, request_count)| ActiveModelRuntime {
                model,
                request_count,
            })
            .collect()
    }

    pub(crate) fn runtime_activity_for(
        &self,
        candidate_id: &str,
    ) -> (u32, u32, Vec<ActiveModelRuntime>) {
        (
            self.in_flight_count(candidate_id, InFlightLane::Text),
            self.active_request_count(candidate_id),
            self.active_models_for(candidate_id),
        )
    }

    fn dispatch_count(&self, candidate_id: &str, lane: InFlightLane) -> u64 {
        self.activity.dispatch_count(candidate_id, lane)
    }

    pub(super) fn rotation_dispatch_count(
        &self,
        candidate: &RuntimeCandidate,
        lane: InFlightLane,
    ) -> u64 {
        let dispatches = self.dispatch_count(&candidate.id, lane);
        if candidate.kind == CandidateKind::ApiSource {
            // API providers can safely handle concurrent requests. A reservation
            // must not count toward source rotation until it has settled, or an
            // unrelated active chat would move subsequent chats to another API.
            dispatches.saturating_sub(u64::from(self.in_flight_count(&candidate.id, lane)))
        } else {
            dispatches
        }
    }

    fn quota_reserve_allows(&self, candidate: &RuntimeCandidate, now_ms: u64) -> bool {
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

pub fn normalize_subscription_plan_order(
    values: Vec<String>,
) -> std::result::Result<Vec<String>, &'static str> {
    if values.len() > 64 {
        return Err("subscription plan order contains too many groups");
    }
    let mut seen = HashSet::new();
    let mut normalized = Vec::with_capacity(values.len());
    for value in values {
        let value = value.trim().to_lowercase();
        if value.is_empty() || value.len() > 64 || value.chars().any(char::is_control) {
            return Err("subscription plan group is invalid");
        }
        if seen.insert(value.clone()) {
            normalized.push(value);
        }
    }
    Ok(normalized)
}

fn subscription_plan_key(plan_type: Option<&str>) -> String {
    plan_type
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_lowercase)
        .unwrap_or_else(|| "unknown".to_string())
}

fn half_open_scope(candidate: &RuntimeCandidate, model: &str, now_ms: u64) -> Option<String> {
    candidate
        .cooldowns
        .get("*")
        .filter(|retry_at_ms| **retry_at_ms <= now_ms)
        .map(|_| "*".to_string())
        .or_else(|| {
            has_expired_cooldown(&candidate.cooldowns, model, now_ms)
                .then(|| model.to_ascii_lowercase())
        })
}

#[cfg(test)]
mod tests;
