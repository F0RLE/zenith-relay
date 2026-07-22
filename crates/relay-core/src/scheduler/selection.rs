use super::affinity::AffinityCache;
use super::candidate::{CandidateHealth, CandidateKind, CandidateScope, RuntimeCandidate};
use super::capacity::{CandidateQuota, QUOTA_STALE_AFTER_MS};
use super::cooldown::has_expired_cooldown;
use crate::WireApi;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet, VecDeque};

const API_SOURCE_PRIMARY_PRIORITY: i32 = 1_000_000;
const API_SOURCE_RESERVE_PRIORITY: i32 = -1_000_000;
const RESPONSE_AFFINITY_MAX_ENTRIES: usize = 4_096;
pub const RESPONSE_AFFINITY_TTL_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
const PROMPT_AFFINITY_MAX_ENTRIES: usize = 4_096;
pub const PROMPT_AFFINITY_TTL_MS: u64 = 60 * 60 * 1_000;
const PROMPT_AFFINITY_QUOTA_SLACK_BPS: u64 = 500;
const QUOTA_RESET_TIE_BPS: u64 = 100;
const MAX_OAUTH_IMAGE_IN_FLIGHT: u32 = 1;
const PROVIDER_STORM_WINDOW_MS: u64 = 10_000;
const PROVIDER_STORM_OPEN_MS: u64 = 30_000;
const PROVIDER_STORM_THRESHOLD: usize = 3;

#[derive(Clone, Copy, Eq, PartialEq)]
enum InFlightLane {
    Text,
    Image,
}

pub struct SelectionRequest<'a> {
    pub model: &'a str,
    pub allowed_protocols: &'a [WireApi],
    pub scope: &'a CandidateScope,
    pub tried: &'a HashSet<String>,
    pub response_affinity_key: Option<&'a str>,
    pub prompt_affinity_key: Option<&'a str>,
    pub now_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    pub candidate_id: String,
    pub response_affinity_hit: bool,
    pub half_open_probe: bool,
    pub diagnostics: RoutingDiagnostics,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateRuntimeSnapshot {
    pub candidate_id: String,
    pub kind: CandidateKind,
    pub available: bool,
    pub in_flight: u32,
    pub last_used_at_ms: Option<u64>,
    pub next_retry_at_ms: Option<u64>,
    pub half_open: bool,
    pub dispatches: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    #[default]
    Adaptive,
    QuotaHighest,
    SubscriptionExpiry,
    SubscriptionPlan,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReason {
    ResponseAffinity,
    PromptCacheAffinity,
    SessionAffinity,
    ConnectionAffinity,
    OnlyEligible,
    RoutingTier,
    ParallelLoad,
    PoolPolicy,
    QuotaHeadroom,
    AdaptiveBalance,
    SubscriptionExpiry,
    SubscriptionPlan,
    FairRotation,
    LeastRecentlyUsed,
    ManualPriority,
    ManualWeight,
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
}

#[derive(Clone, Debug)]
pub struct PoolScheduler {
    candidates: BTreeMap<String, RuntimeCandidate>,
    response_affinity: AffinityCache,
    prompt_affinity: AffinityCache,
    in_flight: BTreeMap<String, u32>,
    image_in_flight: BTreeMap<String, u32>,
    half_open: BTreeSet<(String, String)>,
    dispatches: BTreeMap<String, u64>,
    image_dispatches: BTreeMap<String, u64>,
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
            response_affinity: AffinityCache::new(
                RESPONSE_AFFINITY_MAX_ENTRIES,
                RESPONSE_AFFINITY_TTL_MS,
            ),
            prompt_affinity: AffinityCache::new(
                PROMPT_AFFINITY_MAX_ENTRIES,
                PROMPT_AFFINITY_TTL_MS,
            ),
            in_flight: BTreeMap::new(),
            image_in_flight: BTreeMap::new(),
            half_open: BTreeSet::new(),
            dispatches: BTreeMap::new(),
            image_dispatches: BTreeMap::new(),
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
        }
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
            self.dispatches.clear();
            self.image_dispatches.clear();
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

    pub fn upsert(&mut self, mut candidate: RuntimeCandidate) {
        if candidate.kind == CandidateKind::OAuthAccount {
            candidate.cooldowns.clear();
            candidate.consecutive_failures = 0;
        }
        self.candidates.insert(candidate.id.clone(), candidate);
    }

    pub fn remove(&mut self, candidate_id: &str) -> Option<RuntimeCandidate> {
        self.response_affinity.invalidate_candidate(candidate_id);
        self.prompt_affinity.invalidate_candidate(candidate_id);
        self.in_flight.remove(candidate_id);
        self.image_in_flight.remove(candidate_id);
        self.half_open
            .retain(|(half_open_candidate, _)| half_open_candidate != candidate_id);
        self.dispatches.remove(candidate_id);
        self.image_dispatches.remove(candidate_id);
        self.subscription_expires_at_ms.remove(candidate_id);
        self.subscription_plans.remove(candidate_id);
        self.execution_fences.remove(candidate_id);
        self.capability_blocks
            .retain(|(blocked_candidate, _)| blocked_candidate != candidate_id);
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
            .map(
                |(candidate, available, in_flight)| CandidateRuntimeSnapshot {
                    candidate_id: candidate.id.clone(),
                    kind: candidate.kind,
                    available,
                    in_flight,
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
                    dispatches: self
                        .dispatches
                        .get(&candidate.id)
                        .copied()
                        .unwrap_or_default(),
                },
            )
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
            self.dispatches.clear();
            self.image_dispatches.clear();
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
            self.dispatches.clear();
            self.image_dispatches.clear();
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
                    && self.prompt_affinity_allows(preferred, baseline, lane)
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
        lane == InFlightLane::Text
            || candidate.kind != CandidateKind::OAuthAccount
            || self
                .image_in_flight
                .get(&candidate.id)
                .copied()
                .unwrap_or_default()
                < MAX_OAUTH_IMAGE_IN_FLIGHT
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

    pub fn has_response_affinity(&mut self, key: &str, now_ms: u64) -> bool {
        self.response_affinity.contains(key, now_ms)
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
        let in_flight = self
            .in_flight_map_mut(lane)
            .entry(candidate_id.to_string())
            .or_default();
        *in_flight = in_flight.saturating_add(1);
        let dispatches = self
            .dispatch_map_mut(lane)
            .entry(candidate_id.to_string())
            .or_default();
        *dispatches = dispatches.saturating_add(1);
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
        let in_flight_map = self.in_flight_map_mut(lane);
        let Some(in_flight) = in_flight_map.get_mut(candidate_id) else {
            return false;
        };
        if *in_flight <= 1 {
            in_flight_map.remove(candidate_id);
        } else {
            *in_flight -= 1;
        }
        true
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
        self.provider_storm_breakers.remove(&provider_key);
        recovered
    }

    pub fn record_failure(&mut self, candidate_id: &str) -> Option<u32> {
        let candidate = self.candidates.get_mut(candidate_id)?;
        if candidate.kind == CandidateKind::OAuthAccount {
            return Some(0);
        }
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
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        if candidate.kind == CandidateKind::OAuthAccount {
            return false;
        }
        let scope = if model == "*" {
            "*".to_string()
        } else {
            model.to_ascii_lowercase()
        };
        candidate
            .cooldowns
            .entry(scope.clone())
            .and_modify(|current| *current = (*current).max(retry_at_ms))
            .or_insert(retry_at_ms);
        if scope == "*" {
            self.half_open
                .retain(|(half_open_candidate, _)| half_open_candidate != candidate_id);
        } else {
            self.half_open.remove(&(candidate_id.to_string(), scope));
        }
        true
    }

    pub fn clear_cooldown(&mut self, candidate_id: &str, model: &str) -> bool {
        self.candidates
            .get_mut(candidate_id)
            .is_some_and(|candidate| {
                let previous_len = candidate.cooldowns.len();
                candidate
                    .cooldowns
                    .retain(|candidate_model, _| !candidate_model.eq_ignore_ascii_case(model));
                candidate.cooldowns.len() != previous_len
            })
    }

    fn compare_preference(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
        lane: InFlightLane,
    ) -> Ordering {
        let left_in_flight = self.in_flight_count(&left.id, lane);
        let right_in_flight = self.in_flight_count(&right.id, lane);
        let left_dispatches = self.dispatch_count(&left.id, lane);
        let right_dispatches = self.dispatch_count(&right.id, lane);
        let common = routing_tier(left)
            .cmp(&routing_tier(right))
            .then_with(|| right_in_flight.cmp(&left_in_flight))
            .then_with(|| candidate_kind_preference(left).cmp(&candidate_kind_preference(right)));
        match self.routing_strategy {
            RoutingStrategy::Adaptive => routing_tier(left)
                .cmp(&routing_tier(right))
                .then_with(|| {
                    candidate_kind_preference(left).cmp(&candidate_kind_preference(right))
                })
                .then_with(|| self.compare_quota_and_reset(left, right))
                .then_with(|| right_in_flight.cmp(&left_in_flight))
                .then_with(|| {
                    self.compare_equal_quota_rotation(
                        left,
                        right,
                        left_dispatches,
                        right_dispatches,
                    )
                })
                .then_with(|| right.id.cmp(&left.id)),
            RoutingStrategy::QuotaHighest => routing_tier(left)
                .cmp(&routing_tier(right))
                .then_with(|| {
                    candidate_kind_preference(left).cmp(&candidate_kind_preference(right))
                })
                .then_with(|| self.compare_quota_and_reset(left, right))
                .then_with(|| right.id.cmp(&left.id)),
            RoutingStrategy::SubscriptionExpiry => common
                .then_with(|| self.compare_subscription_expiry(left, right))
                .then_with(|| self.compare_quota_and_reset(left, right))
                .then_with(|| {
                    self.compare_equal_quota_rotation(
                        left,
                        right,
                        left_dispatches,
                        right_dispatches,
                    )
                })
                .then_with(|| right.id.cmp(&left.id)),
            RoutingStrategy::SubscriptionPlan => common
                .then_with(|| self.compare_subscription_plan(left, right))
                .then_with(|| self.compare_quota_and_reset(left, right))
                .then_with(|| {
                    self.compare_equal_quota_rotation(
                        left,
                        right,
                        left_dispatches,
                        right_dispatches,
                    )
                })
                .then_with(|| right.id.cmp(&left.id)),
        }
    }

    fn prompt_affinity_allows(
        &self,
        preferred: &RuntimeCandidate,
        baseline: &RuntimeCandidate,
        lane: InFlightLane,
    ) -> bool {
        if routing_tier(preferred) != routing_tier(baseline)
            || candidate_kind_preference(preferred) != candidate_kind_preference(baseline)
            || self.in_flight_count(&preferred.id, lane) != self.in_flight_count(&baseline.id, lane)
        {
            return false;
        }
        match (self.routing_quota(preferred), self.routing_quota(baseline)) {
            (CandidateQuota::Available(preferred), CandidateQuota::Available(baseline)) => {
                preferred.saturating_add(PROMPT_AFFINITY_QUOTA_SLACK_BPS) >= baseline
            }
            (CandidateQuota::Available(_), CandidateQuota::Unknown)
            | (CandidateQuota::Unknown, CandidateQuota::Unknown) => true,
            _ => false,
        }
    }

    fn compare_quota_and_reset(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
    ) -> Ordering {
        match (self.routing_quota(left), self.routing_quota(right)) {
            (CandidateQuota::Available(left_quota), CandidateQuota::Available(right_quota))
                if left_quota.abs_diff(right_quota) <= QUOTA_RESET_TIE_BPS =>
            {
                match (left.quota_reset_at_ms, right.quota_reset_at_ms) {
                    (Some(left_reset), Some(right_reset)) => right_reset
                        .cmp(&left_reset)
                        .then_with(|| left_quota.cmp(&right_quota)),
                    _ => left_quota.cmp(&right_quota),
                }
            }
            (left_quota, right_quota) => left_quota.compare_preference(right_quota),
        }
    }

    fn provider_storm_open(&self, candidate: &RuntimeCandidate, model: &str, now_ms: u64) -> bool {
        self.provider_storm_breaker_enabled
            && self
                .provider_storm_breakers
                .get(&(candidate.source_id.clone(), model.to_ascii_lowercase()))
                .is_some_and(|breaker| breaker.open_until_ms > now_ms)
    }

    fn selection_reason(
        &self,
        selected: &RuntimeCandidate,
        runner_up: &RuntimeCandidate,
        lane: InFlightLane,
    ) -> SelectionReason {
        let selected_in_flight = self.in_flight_count(&selected.id, lane);
        let runner_up_in_flight = self.in_flight_count(&runner_up.id, lane);
        let selected_dispatches = self.dispatch_count(&selected.id, lane);
        let runner_up_dispatches = self.dispatch_count(&runner_up.id, lane);
        if routing_tier(selected) != routing_tier(runner_up) {
            SelectionReason::RoutingTier
        } else if candidate_kind_preference(selected) != candidate_kind_preference(runner_up) {
            SelectionReason::PoolPolicy
        } else if matches!(
            self.routing_strategy,
            RoutingStrategy::SubscriptionExpiry | RoutingStrategy::SubscriptionPlan
        ) && selected_in_flight != runner_up_in_flight
        {
            SelectionReason::ParallelLoad
        } else if self.routing_strategy == RoutingStrategy::SubscriptionExpiry
            && self.compare_subscription_expiry(selected, runner_up) != Ordering::Equal
        {
            SelectionReason::SubscriptionExpiry
        } else if self.routing_strategy == RoutingStrategy::SubscriptionPlan
            && self.compare_subscription_plan(selected, runner_up) != Ordering::Equal
        {
            SelectionReason::SubscriptionPlan
        } else if self
            .routing_quota(selected)
            .compare_preference(self.routing_quota(runner_up))
            != Ordering::Equal
        {
            SelectionReason::QuotaHeadroom
        } else if self.routing_strategy == RoutingStrategy::Adaptive
            && selected_in_flight != runner_up_in_flight
        {
            SelectionReason::ParallelLoad
        } else if self.routing_strategy != RoutingStrategy::QuotaHighest
            && self.compare_equal_quota_rotation(
                selected,
                runner_up,
                selected_dispatches,
                runner_up_dispatches,
            ) != Ordering::Equal
        {
            SelectionReason::FairRotation
        } else {
            SelectionReason::StableTieBreak
        }
    }

    fn in_flight_count(&self, candidate_id: &str, lane: InFlightLane) -> u32 {
        self.in_flight_map(lane)
            .get(candidate_id)
            .copied()
            .unwrap_or_default()
    }

    fn dispatch_count(&self, candidate_id: &str, lane: InFlightLane) -> u64 {
        self.dispatch_map(lane)
            .get(candidate_id)
            .copied()
            .unwrap_or_default()
    }

    fn in_flight_map(&self, lane: InFlightLane) -> &BTreeMap<String, u32> {
        match lane {
            InFlightLane::Text => &self.in_flight,
            InFlightLane::Image => &self.image_in_flight,
        }
    }

    fn in_flight_map_mut(&mut self, lane: InFlightLane) -> &mut BTreeMap<String, u32> {
        match lane {
            InFlightLane::Text => &mut self.in_flight,
            InFlightLane::Image => &mut self.image_in_flight,
        }
    }

    fn dispatch_map(&self, lane: InFlightLane) -> &BTreeMap<String, u64> {
        match lane {
            InFlightLane::Text => &self.dispatches,
            InFlightLane::Image => &self.image_dispatches,
        }
    }

    fn dispatch_map_mut(&mut self, lane: InFlightLane) -> &mut BTreeMap<String, u64> {
        match lane {
            InFlightLane::Text => &mut self.dispatches,
            InFlightLane::Image => &mut self.image_dispatches,
        }
    }

    fn compare_equal_quota_rotation(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
        left_dispatches: u64,
        right_dispatches: u64,
    ) -> Ordering {
        if left.kind == CandidateKind::ApiSource && right.kind == CandidateKind::ApiSource {
            return compare_projected_weighted_values(
                left_dispatches,
                right_dispatches,
                u128::from(left.weight.max(1)),
                u128::from(right.weight.max(1)),
            );
        }
        right_dispatches.cmp(&left_dispatches)
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

    fn routing_quota_factor(&self, candidate: &RuntimeCandidate) -> u64 {
        let reserve = self
            .protected_candidate
            .as_ref()
            .filter(|(candidate_id, _)| candidate_id == &candidate.id)
            .map_or(0, |(_, reserve)| *reserve);
        match candidate.quota {
            CandidateQuota::Available(remaining) => remaining.saturating_sub(reserve),
            CandidateQuota::Unknown if reserve == 0 => 1,
            CandidateQuota::Unknown | CandidateQuota::Exhausted | CandidateQuota::Stale => 0,
        }
    }

    fn routing_quota(&self, candidate: &RuntimeCandidate) -> CandidateQuota {
        match candidate.quota {
            CandidateQuota::Available(_) => {
                CandidateQuota::Available(self.routing_quota_factor(candidate))
            }
            quota => quota,
        }
    }

    fn compare_subscription_expiry(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
    ) -> Ordering {
        if left.kind != CandidateKind::OAuthAccount || right.kind != CandidateKind::OAuthAccount {
            return Ordering::Equal;
        }
        match (
            self.subscription_expires_at_ms.get(&left.id),
            self.subscription_expires_at_ms.get(&right.id),
        ) {
            (Some(left), Some(right)) => right.cmp(left),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => Ordering::Equal,
        }
    }

    fn compare_subscription_plan(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
    ) -> Ordering {
        if left.kind != CandidateKind::OAuthAccount || right.kind != CandidateKind::OAuthAccount {
            return Ordering::Equal;
        }
        let rank = |candidate: &RuntimeCandidate| {
            self.subscription_plans
                .get(&candidate.id)
                .and_then(|plan| self.subscription_plan_ranks.get(plan))
                .copied()
                .unwrap_or(usize::MAX)
        };
        rank(right).cmp(&rank(left))
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

fn routing_tier(candidate: &RuntimeCandidate) -> i8 {
    match candidate.kind {
        CandidateKind::OAuthAccount => 0,
        CandidateKind::ApiSource if candidate.priority >= API_SOURCE_PRIMARY_PRIORITY => 1,
        CandidateKind::ApiSource if candidate.priority <= API_SOURCE_RESERVE_PRIORITY => -1,
        CandidateKind::ApiSource => 0,
    }
}

fn candidate_kind_preference(candidate: &RuntimeCandidate) -> u8 {
    u8::from(candidate.kind == CandidateKind::OAuthAccount)
}

fn compare_weighted_values(
    left_value: u64,
    right_value: u64,
    left_weight: u128,
    right_weight: u128,
) -> Ordering {
    (u128::from(right_value) * left_weight).cmp(&(u128::from(left_value) * right_weight))
}

fn compare_projected_weighted_values(
    left_value: u64,
    right_value: u64,
    left_weight: u128,
    right_weight: u128,
) -> Ordering {
    compare_weighted_values(
        left_value.saturating_add(1),
        right_value.saturating_add(1),
        left_weight,
        right_weight,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scheduler::{CandidateKind, CandidateQuota};
    use crate::ModelRules;
    use std::collections::{BTreeSet, HashSet};

    fn candidate(id: &str) -> RuntimeCandidate {
        RuntimeCandidate {
            id: id.to_string(),
            kind: CandidateKind::ApiSource,
            source_id: id.to_string(),
            account_id: None,
            protocol: WireApi::Responses,
            enabled: true,
            draining: false,
            priority: 0,
            weight: 1,
            models: ["gpt-5".to_string()].into(),
            model_rules: ModelRules::default(),
            health: CandidateHealth::Healthy,
            quota: CandidateQuota::Unknown,
            quota_updated_at_ms: None,
            quota_reset_at_ms: None,
            cooldowns: BTreeMap::new(),
            last_used_at: None,
            consecutive_failures: 0,
            secret_available: true,
        }
    }

    fn oauth_candidate(id: &str) -> RuntimeCandidate {
        RuntimeCandidate {
            kind: CandidateKind::OAuthAccount,
            account_id: Some(id.to_string()),
            ..candidate(id)
        }
    }

    fn select(scheduler: &mut PoolScheduler, tried: &HashSet<String>) -> Option<Selection> {
        scheduler.select(SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
            scope: &CandidateScope::default(),
            tried,
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        })
    }

    fn select_image(scheduler: &mut PoolScheduler, tried: &HashSet<String>) -> Option<Selection> {
        scheduler.select_image(SelectionRequest {
            model: "gpt-image-2",
            allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
            scope: &CandidateScope::default(),
            tried,
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 100,
        })
    }

    #[test]
    fn image_lane_is_separate_from_text_load_and_caps_each_oauth_account() {
        let mut first = oauth_candidate("first");
        first.models.insert("gpt-image-2".to_string());
        let mut second = oauth_candidate("second");
        second.models.insert("gpt-image-2".to_string());
        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(first);
        scheduler.upsert(second);

        assert!(scheduler.reserve_for("first", "gpt-5", 100));
        let image = select_image(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(image.candidate_id, "first");
        assert_eq!(image.diagnostics.in_flight_before, 0);
        assert!(scheduler.reserve_image_for("first", "gpt-image-2", 100));

        let next_image = select_image(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(next_image.candidate_id, "second");
        let text = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(text.candidate_id, "second");
        assert_eq!(text.diagnostics.in_flight_before, 0);

        assert!(scheduler.release_image_for("first", Some("gpt-image-2")));
        assert!(scheduler.release_for("first", Some("gpt-5")));
    }

    #[test]
    fn availability_updates_take_effect_while_candidate_is_in_flight() {
        let mut scheduler = PoolScheduler::new();
        let first = oauth_candidate("first");
        let second = oauth_candidate("second");
        scheduler.upsert(first);
        scheduler.upsert(second);
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "first"
        );
        assert!(scheduler.reserve("first"));

        assert!(scheduler.update_candidate_availability(
            "first",
            true,
            CandidateHealth::Healthy,
            CandidateQuota::Exhausted,
        ));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "second"
        );
        assert!(scheduler.release("first"));
        assert!(!scheduler.update_candidate_availability(
            "missing",
            true,
            CandidateHealth::Healthy,
            CandidateQuota::Unknown,
        ));
        assert!(scheduler.set_candidate_health("second", CandidateHealth::Unhealthy));
        assert!(!scheduler.set_candidate_health("missing", CandidateHealth::Healthy));
        assert!(select(&mut scheduler, &HashSet::new()).is_none());
    }

    #[test]
    fn stale_oauth_quota_stays_probeable_unless_it_protects_the_chatgpt_reserve() {
        let mut scheduler = PoolScheduler::new();
        let mut account = oauth_candidate("account");
        account.quota = CandidateQuota::Available(5_000);
        account.quota_updated_at_ms = Some(100);
        scheduler.upsert(account);
        let scope = CandidateScope::default();
        let tried = HashSet::new();
        let request = |now_ms| SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &tried,
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms,
        };

        assert!(scheduler
            .select(request(100 + QUOTA_STALE_AFTER_MS))
            .is_some());
        assert!(scheduler
            .select(request(101 + QUOTA_STALE_AFTER_MS))
            .is_some());
        assert!(scheduler.set_protected_candidate(Some("account"), 100));
        assert!(scheduler
            .select(request(101 + QUOTA_STALE_AFTER_MS))
            .is_none());
    }

    #[test]
    fn hard_filters_reject_every_ineligible_candidate_state() {
        let mut candidates = Vec::new();

        let mut disabled = candidate("disabled");
        disabled.enabled = false;
        candidates.push(disabled);
        let mut draining = candidate("draining");
        draining.draining = true;
        candidates.push(draining);
        let mut no_secret = candidate("no-secret");
        no_secret.secret_available = false;
        candidates.push(no_secret);
        let mut wrong_model = candidate("wrong-model");
        wrong_model.models = ["other".to_string()].into();
        candidates.push(wrong_model);
        let mut excluded_model = candidate("excluded-model");
        excluded_model.model_rules.excluded = ["gpt-*".to_string()].into();
        candidates.push(excluded_model);
        let mut unhealthy = candidate("unhealthy");
        unhealthy.health = CandidateHealth::Unhealthy;
        candidates.push(unhealthy);
        for (id, health) in [
            ("reauth", CandidateHealth::ReauthRequired),
            ("checkpoint", CandidateHealth::Checkpoint),
            ("captcha", CandidateHealth::Captcha),
            ("blocked", CandidateHealth::Blocked),
            ("expired", CandidateHealth::Expired),
        ] {
            let mut blocked = candidate(id);
            blocked.health = health;
            candidates.push(blocked);
        }
        let mut exhausted = candidate("exhausted");
        exhausted.quota = CandidateQuota::Exhausted;
        candidates.push(exhausted);
        let mut zero_quota = candidate("zero-quota");
        zero_quota.quota = CandidateQuota::Available(0);
        candidates.push(zero_quota);
        let mut stale = candidate("stale");
        stale.quota = CandidateQuota::Stale;
        candidates.push(stale);
        let mut cooled = candidate("cooled");
        cooled.cooldowns.insert("gpt-5".to_string(), 101);
        candidates.push(cooled);
        let mut wrong_protocol = candidate("wrong-protocol");
        wrong_protocol.protocol = WireApi::Messages;
        candidates.push(wrong_protocol);

        let mut scheduler = PoolScheduler::new();
        for candidate in candidates {
            scheduler.upsert(candidate);
        }
        assert_eq!(select(&mut scheduler, &HashSet::new()), None);

        scheduler.upsert(candidate("ready"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "ready"
        );
        let scope = CandidateScope {
            source_ids: Some(["different-source".to_string()].into()),
            ..CandidateScope::default()
        };
        assert_eq!(
            scheduler.select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 100,
            }),
            None
        );

        let scope = CandidateScope {
            model_rules: ModelRules {
                excluded: ["gpt-*".to_string()].into(),
                ..ModelRules::default()
            },
            ..CandidateScope::default()
        };
        assert!(scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 100,
            })
            .is_none());
    }

    #[test]
    fn selection_orders_quota_then_source_share_and_stable_id() {
        let mut scheduler = PoolScheduler::new();
        let mut low_priority = candidate("a-low-priority");
        low_priority.priority = 1;
        low_priority.quota = CandidateQuota::Available(100);
        scheduler.upsert(low_priority);
        let mut high_priority = candidate("z-high-priority");
        high_priority.priority = 100;
        high_priority.quota = CandidateQuota::Available(100);
        scheduler.upsert(high_priority);
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "a-low-priority"
        );

        scheduler = PoolScheduler::new();
        let mut unknown = candidate("unknown");
        unknown.quota = CandidateQuota::Unknown;
        scheduler.upsert(unknown);
        let mut known_low = candidate("known-low");
        known_low.quota = CandidateQuota::Available(1);
        scheduler.upsert(known_low);
        let mut known_high = candidate("known-high");
        known_high.quota = CandidateQuota::Available(2);
        scheduler.upsert(known_high);
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "known-high"
        );

        scheduler = PoolScheduler::new();
        let mut old = candidate("old");
        old.last_used_at = Some(1);
        scheduler.upsert(old);
        let mut new = candidate("new");
        new.last_used_at = Some(2);
        scheduler.upsert(new);
        scheduler.upsert(candidate("never"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "never"
        );

        scheduler = PoolScheduler::new();
        let mut light = candidate("light");
        light.weight = 1;
        scheduler.upsert(light);
        let mut heavy = candidate("heavy");
        heavy.weight = 2;
        scheduler.upsert(heavy);
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "heavy"
        );

        scheduler = PoolScheduler::new();
        scheduler.upsert(candidate("b"));
        scheduler.upsert(candidate("a"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "a"
        );
    }

    #[test]
    fn oauth_quota_ignores_last_use_and_legacy_priority() {
        let mut scheduler = PoolScheduler::new();
        let mut low_quota = oauth_candidate("low-quota");
        low_quota.quota = CandidateQuota::Available(1);
        low_quota.priority = 100;
        low_quota.last_used_at = None;
        scheduler.upsert(low_quota);
        let mut high_quota = oauth_candidate("high-quota");
        high_quota.quota = CandidateQuota::Available(9_000);
        high_quota.priority = 1;
        high_quota.last_used_at = Some(99);
        scheduler.upsert(high_quota);

        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "high-quota"
        );
    }

    #[test]
    fn prompt_cache_affinity_applies_to_accounts_with_quota_and_load_guards() {
        let mut scheduler = PoolScheduler::new();
        let mut cached = oauth_candidate("cached");
        cached.quota = CandidateQuota::Available(5_000);
        scheduler.upsert(cached);
        let mut fullest = oauth_candidate("fullest");
        fullest.quota = CandidateQuota::Available(5_400);
        scheduler.upsert(fullest);
        assert!(scheduler.bind_prompt_affinity("thread", "cached", 0));

        let select_thread = |scheduler: &mut PoolScheduler| {
            scheduler
                .select(SelectionRequest {
                    model: "gpt-5",
                    allowed_protocols: &[WireApi::Responses],
                    scope: &CandidateScope::default(),
                    tried: &HashSet::new(),
                    response_affinity_key: None,
                    prompt_affinity_key: Some("thread"),
                    now_ms: 1,
                })
                .unwrap()
        };

        let selected = select_thread(&mut scheduler);
        assert_eq!(selected.candidate_id, "cached");
        assert_eq!(
            selected.diagnostics.reason,
            SelectionReason::PromptCacheAffinity
        );

        assert!(scheduler.reserve("cached"));
        assert_eq!(select_thread(&mut scheduler).candidate_id, "fullest");
        assert!(scheduler.release("cached"));

        assert!(scheduler.update_candidate_availability(
            "fullest",
            true,
            CandidateHealth::Healthy,
            CandidateQuota::Available(5_501),
        ));
        assert_eq!(select_thread(&mut scheduler).candidate_id, "fullest");

        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(candidate("a"));
        scheduler.upsert(candidate("b"));
        assert!(scheduler.bind_prompt_affinity("thread", "b", 0));
        let selected = select_thread(&mut scheduler);
        assert_eq!(selected.candidate_id, "b");
        assert_eq!(
            selected.diagnostics.reason,
            SelectionReason::PromptCacheAffinity
        );
    }

    #[test]
    fn oauth_equal_quota_uses_stable_order_without_last_use() {
        let mut scheduler = PoolScheduler::new();
        let mut high_priority = oauth_candidate("high-priority");
        high_priority.priority = 100;
        high_priority.last_used_at = Some(20);
        scheduler.upsert(high_priority);
        let mut low_priority = oauth_candidate("low-priority");
        low_priority.priority = 1;
        low_priority.last_used_at = Some(10);
        scheduler.upsert(low_priority);

        let first = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(first.candidate_id, "high-priority");
        assert!(scheduler.record_success("high-priority", "gpt-5", 30));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "high-priority"
        );
    }

    #[test]
    fn api_source_roles_remain_strict_around_fair_oauth_routing() {
        let scope = CandidateScope::default();
        let tried = HashSet::new();
        let request = |scheduler: &mut PoolScheduler| {
            scheduler
                .select(SelectionRequest {
                    model: "gpt-5",
                    allowed_protocols: &[WireApi::Responses],
                    scope: &scope,
                    tried: &tried,
                    response_affinity_key: None,
                    prompt_affinity_key: None,
                    now_ms: 100,
                })
                .unwrap()
                .candidate_id
        };

        let mut primary = PoolScheduler::new();
        let mut source = candidate("primary-source");
        source.priority = API_SOURCE_PRIMARY_PRIORITY;
        primary.upsert(source);
        primary.upsert(oauth_candidate("account"));
        assert_eq!(request(&mut primary), "primary-source");

        let mut reserve = PoolScheduler::new();
        let mut source = candidate("reserve-source");
        source.priority = API_SOURCE_RESERVE_PRIORITY;
        reserve.upsert(source);
        reserve.upsert(oauth_candidate("account"));
        assert_eq!(request(&mut reserve), "account");

        let mut stabilizer = PoolScheduler::new();
        stabilizer.upsert(candidate("stabilizer-source"));
        stabilizer.upsert(oauth_candidate("account"));
        assert_eq!(request(&mut stabilizer), "account");
        assert!(stabilizer.reserve("account"));
        assert_eq!(request(&mut stabilizer), "account");
    }

    #[test]
    fn active_and_sequential_requests_keep_the_highest_quota() {
        let mut scheduler = PoolScheduler::new();
        let mut full = candidate("full");
        full.quota = CandidateQuota::Available(100);
        scheduler.upsert(full);
        let mut low = candidate("low");
        low.quota = CandidateQuota::Available(1);
        scheduler.upsert(low);

        let first = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(first.candidate_id, "full");
        assert!(scheduler.reserve(&first.candidate_id));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "full"
        );
        assert!(scheduler.release(&first.candidate_id));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "full"
        );
    }

    #[test]
    fn one_account_accepts_multiple_parallel_requests() {
        let mut scheduler = PoolScheduler::new();
        let mut account = oauth_candidate("only");
        account.quota = CandidateQuota::Available(5_000);
        scheduler.upsert(account);

        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "only"
        );
        assert!(scheduler.reserve("only"));
        let second = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(second.candidate_id, "only");
        assert_eq!(second.diagnostics.in_flight_before, 1);
    }

    #[test]
    fn higher_quota_account_remains_preferred_until_refresh() {
        let mut scheduler = PoolScheduler::new();
        let mut full = oauth_candidate("full");
        full.quota = CandidateQuota::Available(100);
        scheduler.upsert(full);
        let mut low = oauth_candidate("low");
        low.quota = CandidateQuota::Available(1);
        scheduler.upsert(low);

        for _ in 0..100 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            assert_eq!(selected.candidate_id, "full");
            assert!(scheduler.reserve(&selected.candidate_id));
            assert!(scheduler.release(&selected.candidate_id));
        }
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "full"
        );
    }

    #[test]
    fn equal_quota_sequential_requests_rotate_without_configured_weight() {
        let mut scheduler = PoolScheduler::new();
        for (id, weight) in [("full", 4), ("half", 2), ("quarter", 1)] {
            let mut account = oauth_candidate(id);
            account.quota = CandidateQuota::Available(5_000);
            account.weight = weight;
            scheduler.upsert(account);
        }

        let mut counts = BTreeMap::new();
        for _ in 0..70 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            assert!(scheduler.reserve(&selected.candidate_id));
            *counts.entry(selected.candidate_id.clone()).or_insert(0_u32) += 1;
            assert!(scheduler.release(&selected.candidate_id));
        }

        assert_eq!(counts.get("full"), Some(&24));
        assert_eq!(counts.get("half"), Some(&23));
        assert_eq!(counts.get("quarter"), Some(&23));
    }

    #[test]
    fn quota_highest_keeps_a_stable_equal_quota_account_despite_load_and_dispatches() {
        let mut scheduler = PoolScheduler::new();
        scheduler.set_routing_strategy(RoutingStrategy::QuotaHighest);
        for id in ["first", "second"] {
            let mut account = oauth_candidate(id);
            account.quota = CandidateQuota::Available(5_000);
            scheduler.upsert(account);
        }

        for _ in 0..3 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            assert_eq!(selected.candidate_id, "first");
            assert_eq!(selected.diagnostics.reason, SelectionReason::StableTieBreak);
            assert!(scheduler.reserve("first"));
            assert_eq!(
                select(&mut scheduler, &HashSet::new())
                    .unwrap()
                    .candidate_id,
                "first"
            );
            assert!(scheduler.release("first"));
        }
    }

    #[test]
    fn sequential_requests_use_the_greatest_refreshed_quota() {
        let mut scheduler = PoolScheduler::new();
        for (id, quota) in [("full", 10_000), ("half", 5_000), ("quarter", 2_500)] {
            let mut account = oauth_candidate(id);
            account.quota = CandidateQuota::Available(quota);
            scheduler.upsert(account);
        }

        let mut counts = BTreeMap::new();
        for index in 0..70 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            if index == 0 {
                assert_eq!(selected.candidate_id, "full");
                assert_eq!(selected.diagnostics.reason, SelectionReason::QuotaHeadroom);
                assert_eq!(selected.diagnostics.eligible_candidates, 3);
                assert_eq!(
                    selected.diagnostics.quota_remaining_basis_points,
                    Some(10_000)
                );
                assert_eq!(selected.diagnostics.in_flight_before, 0);
            }
            assert!(scheduler.reserve(&selected.candidate_id));
            *counts.entry(selected.candidate_id.clone()).or_insert(0_u32) += 1;
            assert!(scheduler.release(&selected.candidate_id));
        }
        assert_eq!(counts.get("full"), Some(&70));
        assert_eq!(counts.get("half"), None);
        assert_eq!(counts.get("quarter"), None);
    }

    #[test]
    fn quota_refresh_rebases_rotation_on_current_headroom() {
        let mut scheduler = PoolScheduler::new();
        for (id, quota) in [("first", 10_000), ("second", 1_000)] {
            let mut account = oauth_candidate(id);
            account.quota = CandidateQuota::Available(quota);
            scheduler.upsert(account);
        }
        for _ in 0..11 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            assert!(scheduler.reserve(&selected.candidate_id));
            assert!(scheduler.release(&selected.candidate_id));
        }

        for id in ["first", "second"] {
            assert!(scheduler.update_candidate_availability(
                id,
                true,
                CandidateHealth::Healthy,
                CandidateQuota::Available(5_000),
            ));
        }

        let first = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(first.candidate_id, "first");
        assert!(scheduler.reserve(&first.candidate_id));
        assert!(scheduler.release(&first.candidate_id));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "second"
        );
    }

    #[test]
    fn concurrent_requests_follow_available_quota_headroom() {
        let mut scheduler = PoolScheduler::new();
        for (id, quota) in [("full", 10_000), ("half", 5_000), ("quarter", 2_500)] {
            let mut account = oauth_candidate(id);
            account.quota = CandidateQuota::Available(quota);
            scheduler.upsert(account);
        }

        let mut counts = BTreeMap::new();
        for _ in 0..7 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            assert!(scheduler.reserve(&selected.candidate_id));
            *counts.entry(selected.candidate_id).or_insert(0_u32) += 1;
        }

        assert_eq!(counts.get("full"), Some(&7));
        assert_eq!(counts.get("half"), None);
        assert_eq!(counts.get("quarter"), None);
    }

    #[test]
    fn concurrent_requests_keep_the_greatest_quota() {
        let mut scheduler = PoolScheduler::new();
        for (id, quota) in [
            ("sixty-three", 6_300),
            ("fifty-four", 5_400),
            ("fifty-two", 5_200),
            ("fifty-one", 5_100),
        ] {
            let mut account = oauth_candidate(id);
            account.quota = CandidateQuota::Available(quota);
            scheduler.upsert(account);
        }

        let mut counts = BTreeMap::new();
        for _ in 0..200 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            assert!(scheduler.reserve(&selected.candidate_id));
            *counts.entry(selected.candidate_id).or_insert(0_u32) += 1;
        }

        assert_eq!(counts, [("sixty-three".to_string(), 200)].into());
    }

    #[test]
    fn subscription_expiry_routing_prefers_unknown_then_nearest_available_expiry() {
        let mut scheduler = PoolScheduler::new();
        scheduler.set_routing_strategy(RoutingStrategy::SubscriptionExpiry);
        for (id, expires_at_ms, quota) in [
            ("unknown", None, CandidateQuota::Available(100)),
            ("nearest", Some(10), CandidateQuota::Available(100)),
            ("later", Some(20), CandidateQuota::Available(10_000)),
            ("disabled-unknown", None, CandidateQuota::Available(10_000)),
            ("exhausted-unknown", None, CandidateQuota::Exhausted),
        ] {
            let mut account = oauth_candidate(id);
            account.quota = quota;
            account.enabled = id != "disabled-unknown";
            scheduler.upsert(account);
            assert!(scheduler.set_candidate_subscription_expiry(id, expires_at_ms));
        }

        assert_eq!(
            scheduler
                .runtime_order(0)
                .into_iter()
                .take(3)
                .map(|candidate| candidate.candidate_id)
                .collect::<Vec<_>>(),
            ["unknown", "nearest", "later"]
        );

        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(selected.candidate_id, "unknown");
        assert_eq!(
            selected.diagnostics.reason,
            SelectionReason::SubscriptionExpiry
        );
        assert!(scheduler.reserve("unknown"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "nearest"
        );
        assert!(scheduler.release("unknown"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "unknown"
        );
    }

    #[test]
    fn subscription_plan_routing_follows_group_order_and_skips_busy_accounts() {
        let mut scheduler = PoolScheduler::new();
        scheduler.set_routing_strategy(RoutingStrategy::SubscriptionPlan);
        scheduler.set_subscription_plan_order(&[
            "business".into(),
            "plus".into(),
            "unknown".into(),
        ]);
        for (id, plan, quota) in [
            ("business", Some("Business"), 100),
            ("plus", Some("plus"), 10_000),
            ("unknown", None, 10_000),
        ] {
            let mut account = oauth_candidate(id);
            account.quota = CandidateQuota::Available(quota);
            scheduler.upsert(account);
            assert!(scheduler.set_candidate_subscription_plan(id, plan));
        }

        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(selected.candidate_id, "business");
        assert_eq!(
            selected.diagnostics.reason,
            SelectionReason::SubscriptionPlan
        );
        assert!(scheduler.reserve("business"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "plus"
        );
    }

    #[test]
    fn subscription_plan_order_is_normalized_and_bounded() {
        assert_eq!(
            normalize_subscription_plan_order(vec![
                " Business ".into(),
                "business".into(),
                "PLUS".into()
            ])
            .unwrap(),
            ["business", "plus"]
        );
        assert!(normalize_subscription_plan_order(vec!["bad\nplan".into()]).is_err());
    }

    #[test]
    fn response_affinity_is_mandatory() {
        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(candidate("creator"));
        let mut fallback = candidate("fallback");
        fallback.priority = 10;
        scheduler.upsert(fallback);
        assert!(scheduler.bind_response_affinity("response", "creator", 0));

        let scope = CandidateScope::default();
        let empty = HashSet::new();
        let selection = scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &empty,
                response_affinity_key: Some("response"),
                prompt_affinity_key: None,
                now_ms: 1,
            })
            .unwrap();
        assert_eq!(selection.candidate_id, "creator");
        assert!(selection.response_affinity_hit);
        assert_eq!(
            selection.diagnostics.reason,
            SelectionReason::ResponseAffinity
        );

        scheduler.set_cooldown("creator", "gpt-5", 10);
        assert_eq!(
            scheduler.select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &empty,
                response_affinity_key: Some("response"),
                prompt_affinity_key: None,
                now_ms: 1,
            }),
            None,
            "a continuation cannot move to a candidate that did not create the response"
        );
    }

    #[test]
    fn cooldown_expires_and_success_clears_it_and_updates_last_used_timestamp() {
        let mut scheduler = PoolScheduler::new();
        let mut candidate = candidate("candidate");
        candidate.cooldowns.insert("gpt-5".to_string(), 101);
        candidate.cooldowns.insert("*".to_string(), 101);
        scheduler.upsert(candidate);
        assert_eq!(select(&mut scheduler, &HashSet::new()), None);

        assert!(!scheduler.record_success("candidate", "GPT-5", 90));
        assert_eq!(
            scheduler.candidate("candidate").unwrap().last_used_at,
            Some(90)
        );
        assert_eq!(
            scheduler
                .candidate("candidate")
                .unwrap()
                .cooldowns
                .get("gpt-5"),
            Some(&101)
        );
        assert!(scheduler.record_success("candidate", "GPT-5", 102));
        assert!(scheduler
            .candidate("candidate")
            .unwrap()
            .cooldowns
            .is_empty());
        assert!(select(&mut scheduler, &HashSet::new()).is_some());

        scheduler.set_cooldown("candidate", "gpt-5", 101);
        assert!(scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 101,
            })
            .is_some());
        assert_eq!(scheduler.record_failure("candidate"), Some(1));
        assert_eq!(scheduler.record_failure("candidate"), Some(2));
        assert!(scheduler.record_success("candidate", "gpt-5", 102));
        assert_eq!(
            scheduler
                .candidate("candidate")
                .unwrap()
                .consecutive_failures,
            0
        );
    }

    #[test]
    fn cooldown_updates_never_shorten_an_existing_retry_window() {
        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(candidate("candidate"));
        assert!(scheduler.set_cooldown("candidate", "gpt-5", 10_000));
        assert!(scheduler.set_cooldown("candidate", "GPT-5", 2_000));
        assert_eq!(
            scheduler
                .candidate("candidate")
                .unwrap()
                .cooldowns
                .get("gpt-5"),
            Some(&10_000)
        );
        assert_eq!(scheduler.record_failure("candidate"), Some(1));
        assert!(scheduler.reset_failures("candidate"));
        assert_eq!(
            scheduler
                .candidate("candidate")
                .unwrap()
                .consecutive_failures,
            0
        );
    }

    #[test]
    fn affinity_retry_time_uses_only_the_response_owner() {
        let mut scheduler = PoolScheduler::new();
        let mut owner = candidate("owner");
        owner.cooldowns.insert("gpt-5".into(), 300);
        scheduler.upsert(owner);
        let mut other = candidate("other");
        other.cooldowns.insert("gpt-5".into(), 200);
        scheduler.upsert(other);
        assert!(scheduler.bind_response_affinity("response", "owner", 100));

        let scope = CandidateScope::default();
        assert_eq!(
            scheduler.earliest_retry_at(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &HashSet::new(),
                response_affinity_key: Some("response"),
                prompt_affinity_key: None,
                now_ms: 100,
            }),
            Some(300)
        );
    }

    #[test]
    fn expired_cooldown_allows_only_one_half_open_probe_per_model() {
        let mut scheduler = PoolScheduler::new();
        let mut recovering = candidate("recovering");
        recovering.cooldowns.insert("gpt-5".to_string(), 100);
        scheduler.upsert(recovering);
        let scope = CandidateScope::default();
        let tried = HashSet::new();
        let request = || SelectionRequest {
            model: "gpt-5",
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &tried,
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 101,
        };

        let first = scheduler.select(request()).unwrap();
        assert!(first.half_open_probe);
        assert!(scheduler.reserve_for(&first.candidate_id, "gpt-5", 101));
        assert!(scheduler.select(request()).is_none());
        assert!(scheduler.release_for(&first.candidate_id, Some("gpt-5")));
        assert!(scheduler.select(request()).unwrap().half_open_probe);
    }

    #[test]
    fn expired_global_cooldown_allows_only_one_probe_across_models() {
        let mut scheduler = PoolScheduler::new();
        let mut recovering = candidate("recovering");
        recovering.models.insert("gpt-6".to_string());
        recovering.cooldowns.insert("*".to_string(), 100);
        scheduler.upsert(recovering);
        let scope = CandidateScope::default();
        let tried = HashSet::new();
        let request = |model| SelectionRequest {
            model,
            allowed_protocols: &[WireApi::Responses],
            scope: &scope,
            tried: &tried,
            response_affinity_key: None,
            prompt_affinity_key: None,
            now_ms: 101,
        };

        let first = scheduler.select(request("gpt-5")).unwrap();
        assert!(first.half_open_probe);
        assert!(scheduler.reserve_for(&first.candidate_id, "gpt-5", 101));
        assert!(scheduler.select(request("gpt-6")).is_none());
        assert!(scheduler.release_for(&first.candidate_id, Some("gpt-5")));
        assert!(scheduler.select(request("gpt-6")).unwrap().half_open_probe);
    }

    #[test]
    fn runtime_order_uses_scheduler_preference_and_exposes_live_state() {
        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(candidate("first"));
        scheduler.upsert(candidate("second"));

        let initial = scheduler.runtime_order(50);
        assert_eq!(initial[0].candidate_id, "first");
        assert!(initial.iter().all(|candidate| candidate.available));

        assert!(scheduler.reserve_for("first", "gpt-5", 50));
        let loaded = scheduler.runtime_order(50);
        assert_eq!(loaded[0].candidate_id, "first");
        assert_eq!(loaded[0].in_flight, 1);
        assert_eq!(loaded[0].dispatches, 1);
        assert_eq!(loaded[0].last_used_at_ms, None);
        assert_eq!(
            scheduler
                .select(SelectionRequest {
                    model: "gpt-5",
                    allowed_protocols: &[WireApi::Responses],
                    scope: &CandidateScope::default(),
                    tried: &HashSet::new(),
                    response_affinity_key: None,
                    prompt_affinity_key: None,
                    now_ms: 50,
                })
                .unwrap()
                .candidate_id,
            "second"
        );
        assert!(scheduler.record_success("first", "gpt-5", 75));
        assert_eq!(scheduler.runtime_order(75)[0].last_used_at_ms, Some(75));

        assert!(scheduler.set_cooldown("second", "gpt-5", 100));
        let cooling = scheduler.runtime_order(50);
        let second = cooling
            .iter()
            .find(|candidate| candidate.candidate_id == "second")
            .unwrap();
        assert!(!second.available);
        assert_eq!(second.next_retry_at_ms, Some(100));

        assert!(scheduler.reserve_for("second", "gpt-5", 101));
        let probing = scheduler.runtime_order(101);
        let second = probing
            .iter()
            .find(|candidate| candidate.candidate_id == "second")
            .unwrap();
        assert!(second.half_open);
        assert!(!second.available);
    }

    #[test]
    fn earliest_retry_ignores_candidates_blocked_for_non_cooldown_reasons() {
        let mut scheduler = PoolScheduler::new();
        let mut later = candidate("later");
        later.cooldowns.insert("gpt-5".to_string(), 300);
        scheduler.upsert(later);
        let mut sooner = candidate("sooner");
        sooner.cooldowns.insert("gpt-5".to_string(), 200);
        scheduler.upsert(sooner);
        let mut disabled = candidate("disabled");
        disabled.enabled = false;
        disabled.cooldowns.insert("gpt-5".to_string(), 150);
        scheduler.upsert(disabled);

        assert_eq!(
            scheduler.earliest_retry_at(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 100,
            }),
            Some(200)
        );

        let mut exhausted = candidate("exhausted");
        exhausted.quota = CandidateQuota::Exhausted;
        exhausted.cooldowns.insert("*".to_string(), 250);
        scheduler.upsert(exhausted);
        assert_eq!(
            scheduler.earliest_retry_at(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::from(["sooner".to_string()]),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 100,
            }),
            Some(250)
        );
    }

    #[test]
    fn account_scope_allows_oauth_ready_candidate_shape() {
        let mut scheduler = PoolScheduler::new();
        let mut account = candidate("candidate-account");
        account.kind = CandidateKind::OAuthAccount;
        account.source_id = "openai".to_string();
        account.account_id = Some("account-1".to_string());
        scheduler.upsert(account);
        let scope = CandidateScope {
            account_ids: Some(BTreeSet::from(["account-1".to_string()])),
            ..CandidateScope::default()
        };

        assert!(scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 0,
            })
            .is_some());
    }

    #[test]
    fn oauth_candidates_ignore_internal_cooldowns_and_stale_quota() {
        let mut scheduler = PoolScheduler::new();
        let mut account = oauth_candidate("account");
        account.quota = CandidateQuota::Stale;
        account.cooldowns.insert("gpt-5".into(), 10_000);
        account.consecutive_failures = 7;
        scheduler.upsert(account);

        assert!(!scheduler.set_cooldown("account", "gpt-5", 20_000));
        assert_eq!(scheduler.record_failure("account"), Some(0));
        assert!(select(&mut scheduler, &HashSet::new()).is_some());
        let snapshot = scheduler.runtime_order(100).remove(0);
        assert!(snapshot.available);
        assert_eq!(snapshot.next_retry_at_ms, None);
    }

    #[test]
    fn translated_protocols_share_the_same_scheduler() {
        let mut scheduler = PoolScheduler::new();
        let mut candidate = candidate("chat-source");
        candidate.protocol = WireApi::ChatCompletions;
        scheduler.upsert(candidate);

        assert!(scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 0,
            })
            .is_some());
    }

    #[test]
    fn explicit_empty_scope_selects_no_candidates() {
        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(candidate("source"));
        let scope = CandidateScope {
            source_ids: Some(BTreeSet::new()),
            ..CandidateScope::default()
        };

        assert!(scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 0,
            })
            .is_none());
    }

    #[test]
    fn protected_account_keeps_its_quota_reserve() {
        let mut scheduler = PoolScheduler::new();
        let mut protected = oauth_candidate("protected");
        protected.quota = CandidateQuota::Available(100);
        scheduler.upsert(protected);
        let mut available = oauth_candidate("available");
        available.quota = CandidateQuota::Available(5_000);
        scheduler.upsert(available);
        assert!(scheduler.set_protected_candidate(Some("protected"), 100));

        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "available"
        );

        assert!(scheduler.update_candidate_availability(
            "protected",
            true,
            CandidateHealth::Healthy,
            CandidateQuota::Available(200),
        ));
        assert_eq!(
            scheduler.routing_quota_factor(scheduler.candidate("protected").unwrap()),
            100
        );
    }

    #[test]
    fn execution_fences_are_reference_counted_and_capability_blocks_are_model_scoped() {
        let mut scheduler = PoolScheduler::new();
        let mut account = oauth_candidate("account");
        account.models.insert("gpt-5-mini".into());
        scheduler.upsert(account);

        assert!(scheduler.set_execution_fence("account", true));
        assert!(scheduler.set_execution_fence("account", true));
        assert!(select(&mut scheduler, &HashSet::new()).is_none());
        assert!(scheduler.set_execution_fence("account", false));
        assert!(select(&mut scheduler, &HashSet::new()).is_none());
        assert!(scheduler.set_execution_fence("account", false));

        assert!(scheduler.block_capability("account", "gpt-5"));
        assert!(select(&mut scheduler, &HashSet::new()).is_none());
        assert!(scheduler
            .select(SelectionRequest {
                model: "gpt-5-mini",
                allowed_protocols: &[WireApi::Responses],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                response_affinity_key: None,
                prompt_affinity_key: None,
                now_ms: 100,
            })
            .is_some());
        assert!(scheduler.clear_capability_blocks("account"));
        assert!(select(&mut scheduler, &HashSet::new()).is_some());
    }

    #[test]
    fn near_equal_quota_prefers_the_account_that_resets_first() {
        let mut scheduler = PoolScheduler::new();
        scheduler.set_routing_strategy(RoutingStrategy::QuotaHighest);
        let mut earlier = oauth_candidate("earlier");
        earlier.quota = CandidateQuota::Available(5_000);
        earlier.quota_reset_at_ms = Some(1_000);
        let mut later = oauth_candidate("later");
        later.quota = CandidateQuota::Available(5_050);
        later.quota_reset_at_ms = Some(2_000);
        scheduler.upsert(earlier);
        scheduler.upsert(later);

        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "earlier"
        );
    }

    #[test]
    fn provider_model_storm_breaker_is_shared_by_source_and_clears_on_success() {
        let mut scheduler = PoolScheduler::new();
        scheduler.set_provider_storm_breaker_enabled(true);
        let first = candidate("first");
        let mut second = candidate("second");
        second.source_id = first.source_id.clone();
        scheduler.upsert(first);
        scheduler.upsert(second);

        assert!(!scheduler.record_provider_rate_limit("first", "gpt-5", 1));
        assert!(!scheduler.record_provider_rate_limit("first", "gpt-5", 2));
        assert!(scheduler.record_provider_rate_limit("first", "gpt-5", 3));
        assert!(select(&mut scheduler, &HashSet::new()).is_none());
        assert!(scheduler.record_success("first", "gpt-5", 4));
        assert!(select(&mut scheduler, &HashSet::new()).is_some());
    }
}
