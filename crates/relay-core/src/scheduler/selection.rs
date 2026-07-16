use super::affinity::AffinityCache;
use super::candidate::{CandidateHealth, CandidateKind, CandidateScope, RuntimeCandidate};
use super::capacity::CandidateQuota;
use super::cooldown::has_expired_cooldown;
use crate::WireApi;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashSet};

const API_SOURCE_PRIMARY_PRIORITY: i32 = 1_000_000;
const API_SOURCE_RESERVE_PRIORITY: i32 = -1_000_000;
const RESPONSE_AFFINITY_MAX_ENTRIES: usize = 4_096;
pub const RESPONSE_AFFINITY_TTL_MS: u64 = 24 * 60 * 60 * 1_000;
const MAX_THROUGHPUT_MILLI_TOKENS_PER_SECOND: u64 = 1_000_000;
const MIN_THROUGHPUT_OUTPUT_TOKENS: u64 = 16;
const MIN_THROUGHPUT_SAMPLES: u32 = 3;
const SPEED_FACTOR_SCALE: u128 = 1_000;
const MIN_SPEED_FACTOR: u128 = 500;
const MAX_SPEED_FACTOR: u128 = 2_000;
const MAX_OAUTH_IMAGE_IN_FLIGHT: u32 = 1;

#[derive(Clone, Copy, Eq, PartialEq)]
enum InFlightLane {
    Text,
    Image,
}

#[derive(Clone, Copy, Debug)]
struct ThroughputEstimate {
    milli_tokens_per_second: u64,
    samples: u32,
}

pub struct SelectionRequest<'a> {
    pub model: &'a str,
    pub allowed_protocols: &'a [WireApi],
    pub scope: &'a CandidateScope,
    pub tried: &'a HashSet<String>,
    pub response_affinity_key: Option<&'a str>,
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
    pub next_retry_at_ms: Option<u64>,
    pub effective_weight: u64,
    pub half_open: bool,
    pub dispatches: u64,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    #[default]
    Adaptive,
    OldestAccount,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionReason {
    ResponseAffinity,
    SessionAffinity,
    ConnectionAffinity,
    OnlyEligible,
    RoutingTier,
    ParallelLoad,
    PoolPolicy,
    QuotaHeadroom,
    AdaptiveBalance,
    OldestAccount,
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
    pub effective_weight: u64,
    pub in_flight_before: u32,
    pub dispatches_before: u64,
}

#[derive(Clone, Debug)]
pub struct PoolScheduler {
    candidates: BTreeMap<String, RuntimeCandidate>,
    response_affinity: AffinityCache,
    in_flight: BTreeMap<String, u32>,
    image_in_flight: BTreeMap<String, u32>,
    half_open: BTreeSet<(String, String)>,
    dispatches: BTreeMap<String, u64>,
    image_dispatches: BTreeMap<String, u64>,
    routing_strategy: RoutingStrategy,
    created_at_ms: BTreeMap<String, u64>,
    throughput: BTreeMap<String, ThroughputEstimate>,
    throughput_baseline: Option<u64>,
    protected_candidate: Option<(String, u64)>,
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
            in_flight: BTreeMap::new(),
            image_in_flight: BTreeMap::new(),
            half_open: BTreeSet::new(),
            dispatches: BTreeMap::new(),
            image_dispatches: BTreeMap::new(),
            routing_strategy: RoutingStrategy::Adaptive,
            created_at_ms: BTreeMap::new(),
            throughput: BTreeMap::new(),
            throughput_baseline: None,
            protected_candidate: None,
        }
    }

    pub fn set_routing_strategy(&mut self, strategy: RoutingStrategy) {
        if self.routing_strategy != strategy {
            self.routing_strategy = strategy;
            self.dispatches.clear();
            self.image_dispatches.clear();
        }
    }

    pub fn set_candidate_created_at(&mut self, candidate_id: &str, created_at_ms: u64) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        if created_at_ms == 0 {
            self.created_at_ms.remove(candidate_id);
        } else {
            self.created_at_ms
                .insert(candidate_id.to_string(), created_at_ms);
        }
        true
    }

    pub fn upsert(&mut self, candidate: RuntimeCandidate) {
        self.candidates.insert(candidate.id.clone(), candidate);
    }

    pub fn remove(&mut self, candidate_id: &str) -> Option<RuntimeCandidate> {
        self.response_affinity.invalidate_candidate(candidate_id);
        self.in_flight.remove(candidate_id);
        self.image_in_flight.remove(candidate_id);
        self.half_open
            .retain(|(half_open_candidate, _)| half_open_candidate != candidate_id);
        self.dispatches.remove(candidate_id);
        self.image_dispatches.remove(candidate_id);
        self.created_at_ms.remove(candidate_id);
        self.throughput.remove(candidate_id);
        self.recompute_throughput_baseline();
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
            .map(|candidate| (candidate, self.is_runtime_available(candidate, now_ms)))
            .collect::<Vec<_>>();
        candidates.sort_by(|(left, left_available), (right, right_available)| {
            right_available
                .cmp(left_available)
                .then_with(|| self.compare_preference(right, left, InFlightLane::Text))
        });
        candidates
            .into_iter()
            .map(|(candidate, available)| CandidateRuntimeSnapshot {
                candidate_id: candidate.id.clone(),
                kind: candidate.kind,
                available,
                in_flight: self
                    .in_flight
                    .get(&candidate.id)
                    .copied()
                    .unwrap_or_default(),
                next_retry_at_ms: candidate
                    .cooldowns
                    .values()
                    .copied()
                    .filter(|retry_at_ms| *retry_at_ms > now_ms)
                    .min(),
                effective_weight: u64::try_from(self.effective_weight(candidate))
                    .unwrap_or(u64::MAX),
                half_open: self
                    .half_open
                    .iter()
                    .any(|(candidate_id, _)| candidate_id == &candidate.id),
                dispatches: self
                    .dispatches
                    .get(&candidate.id)
                    .copied()
                    .unwrap_or_default(),
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
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.enabled = enabled;
        candidate.health = health;
        candidate.quota = quota;
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
        let selected = eligible
            .iter()
            .copied()
            .max_by(|left, right| self.compare_preference(left, right, lane))?;
        let runner_up = eligible
            .iter()
            .copied()
            .filter(|candidate| candidate.id != selected.id)
            .max_by(|left, right| self.compare_preference(left, right, lane));
        let reason = runner_up.map_or(SelectionReason::OnlyEligible, |runner_up| {
            self.selection_reason(selected, runner_up, lane)
        });
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
            .is_some_and(|candidate| has_expired_cooldown(&candidate.cooldowns, model, now_ms))
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
            effective_weight: u64::try_from(self.effective_weight(candidate)).unwrap_or(u64::MAX),
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

    pub fn earliest_retry_at(&self, request: SelectionRequest<'_>) -> Option<u64> {
        self.candidates
            .values()
            .filter(|candidate| !request.tried.contains(&candidate.id))
            .filter(|candidate| self.quota_reserve_allows(candidate))
            .filter_map(|candidate| {
                candidate.retry_at_if_visible(
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
        self.quota_reserve_allows(candidate)
            && candidate.is_eligible(model, allowed_protocols, scope, now_ms)
            && !self
                .half_open
                .contains(&(candidate.id.clone(), model.to_ascii_lowercase()))
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
        let half_open_key = (candidate_id.to_string(), model.to_ascii_lowercase());
        if !model.is_empty()
            && self
                .candidates
                .get(candidate_id)
                .is_some_and(|candidate| has_expired_cooldown(&candidate.cooldowns, model, now_ms))
            && !self.half_open.insert(half_open_key)
        {
            return false;
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
        output_tokens: Option<u64>,
        latency_ms: u64,
    ) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        self.half_open
            .remove(&(candidate_id.to_string(), model.to_ascii_lowercase()));
        candidate.cooldowns.retain(|candidate_model, _| {
            candidate_model != "*" && !candidate_model.eq_ignore_ascii_case(model)
        });
        candidate.health = CandidateHealth::Healthy;
        candidate.last_used_at = Some(now_ms);
        candidate.consecutive_failures = 0;
        if let (Some(output_tokens @ MIN_THROUGHPUT_OUTPUT_TOKENS..), 1..) =
            (output_tokens, latency_ms)
        {
            let measured = (u128::from(output_tokens) * 1_000_000 / u128::from(latency_ms))
                .clamp(1, u128::from(MAX_THROUGHPUT_MILLI_TOKENS_PER_SECOND))
                as u64;
            let previous = self.throughput.get(candidate_id).copied();
            let estimate = previous.map_or(
                ThroughputEstimate {
                    milli_tokens_per_second: measured,
                    samples: 1,
                },
                |previous| ThroughputEstimate {
                    milli_tokens_per_second: (previous
                        .milli_tokens_per_second
                        .saturating_mul(3)
                        .saturating_add(measured))
                        / 4,
                    samples: previous.samples.saturating_add(1),
                },
            );
            self.throughput.insert(candidate_id.to_string(), estimate);
            self.recompute_throughput_baseline();
        }
        true
    }

    pub fn record_failure(&mut self, candidate_id: &str) -> Option<u32> {
        let candidate = self.candidates.get_mut(candidate_id)?;
        candidate.consecutive_failures = candidate.consecutive_failures.saturating_add(1);
        Some(candidate.consecutive_failures)
    }

    pub fn set_cooldown(&mut self, candidate_id: &str, model: &str, retry_at_ms: u64) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.cooldowns.insert(model.to_string(), retry_at_ms);
        if model == "*" {
            self.half_open
                .retain(|(half_open_candidate, _)| half_open_candidate != candidate_id);
        } else {
            self.half_open
                .remove(&(candidate_id.to_string(), model.to_ascii_lowercase()));
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
            .then_with(|| self.compare_weighted_load(left, right, left_in_flight, right_in_flight))
            .then_with(|| candidate_kind_preference(left).cmp(&candidate_kind_preference(right)));
        match self.routing_strategy {
            RoutingStrategy::Adaptive => common
                .then_with(|| {
                    self.compare_weighted_dispatches(left, right, left_dispatches, right_dispatches)
                })
                .then_with(|| left.quota.compare_preference(right.quota))
                .then_with(|| compare_lru(left.last_used_at, right.last_used_at))
                .then_with(|| left.priority.cmp(&right.priority))
                .then_with(|| left.weight.cmp(&right.weight))
                .then_with(|| self.compare_measured_speed(left, right))
                .then_with(|| right.id.cmp(&left.id)),
            RoutingStrategy::OldestAccount => common
                .then_with(|| self.compare_account_age(left, right))
                .then_with(|| {
                    compare_weighted_values(
                        left_dispatches,
                        right_dispatches,
                        u128::from(left.weight.max(1)),
                        u128::from(right.weight.max(1)),
                    )
                })
                .then_with(|| left.quota.compare_preference(right.quota))
                .then_with(|| compare_lru(left.last_used_at, right.last_used_at))
                .then_with(|| left.priority.cmp(&right.priority))
                .then_with(|| right.id.cmp(&left.id)),
        }
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
        } else if self.compare_weighted_load(
            selected,
            runner_up,
            selected_in_flight,
            runner_up_in_flight,
        ) != Ordering::Equal
        {
            SelectionReason::ParallelLoad
        } else if candidate_kind_preference(selected) != candidate_kind_preference(runner_up) {
            SelectionReason::PoolPolicy
        } else if self.routing_strategy == RoutingStrategy::OldestAccount
            && self.compare_account_age(selected, runner_up) != Ordering::Equal
        {
            SelectionReason::OldestAccount
        } else if self.compare_weighted_dispatches(
            selected,
            runner_up,
            selected_dispatches,
            runner_up_dispatches,
        ) != Ordering::Equal
        {
            if self.routing_strategy == RoutingStrategy::Adaptive
                && self.effective_weight(selected) != self.effective_weight(runner_up)
            {
                SelectionReason::AdaptiveBalance
            } else {
                SelectionReason::FairRotation
            }
        } else if selected.quota.compare_preference(runner_up.quota) != Ordering::Equal {
            SelectionReason::QuotaHeadroom
        } else if compare_lru(selected.last_used_at, runner_up.last_used_at) != Ordering::Equal {
            SelectionReason::LeastRecentlyUsed
        } else if selected.priority != runner_up.priority {
            SelectionReason::ManualPriority
        } else if selected.weight != runner_up.weight {
            SelectionReason::ManualWeight
        } else if self.routing_strategy == RoutingStrategy::Adaptive
            && self.compare_measured_speed(selected, runner_up) != Ordering::Equal
        {
            SelectionReason::AdaptiveBalance
        } else {
            SelectionReason::StableTieBreak
        }
    }

    fn compare_weighted_load(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
        left_in_flight: u32,
        right_in_flight: u32,
    ) -> Ordering {
        let (left_weight, right_weight) = match self.routing_strategy {
            RoutingStrategy::Adaptive => {
                (self.effective_weight(left), self.effective_weight(right))
            }
            RoutingStrategy::OldestAccount => (
                u128::from(left.weight.max(1)),
                u128::from(right.weight.max(1)),
            ),
        };
        compare_weighted_values(
            u64::from(left_in_flight),
            u64::from(right_in_flight),
            left_weight,
            right_weight,
        )
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

    fn compare_weighted_dispatches(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
        left_dispatches: u64,
        right_dispatches: u64,
    ) -> Ordering {
        let (left_weight, right_weight) = match self.routing_strategy {
            RoutingStrategy::Adaptive => {
                (self.effective_weight(left), self.effective_weight(right))
            }
            RoutingStrategy::OldestAccount => (
                u128::from(left.weight.max(1)),
                u128::from(right.weight.max(1)),
            ),
        };
        compare_projected_weighted_values(
            left_dispatches,
            right_dispatches,
            left_weight,
            right_weight,
        )
    }

    fn effective_weight(&self, candidate: &RuntimeCandidate) -> u128 {
        let base =
            u128::from(candidate.weight.max(1)) * u128::from(self.routing_quota_factor(candidate));
        let Some(baseline @ 1..) = self.throughput_baseline else {
            return base;
        };
        let Some(estimate) = self
            .throughput
            .get(&candidate.id)
            .filter(|estimate| estimate.samples >= MIN_THROUGHPUT_SAMPLES)
        else {
            return base;
        };
        let speed_factor = (u128::from(estimate.milli_tokens_per_second) * SPEED_FACTOR_SCALE
            / u128::from(baseline))
        .clamp(MIN_SPEED_FACTOR, MAX_SPEED_FACTOR);
        base.saturating_mul(speed_factor) / SPEED_FACTOR_SCALE
    }

    fn quota_reserve_allows(&self, candidate: &RuntimeCandidate) -> bool {
        let Some((_, reserve)) = self
            .protected_candidate
            .as_ref()
            .filter(|(candidate_id, _)| candidate_id == &candidate.id)
        else {
            return true;
        };
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

    fn compare_measured_speed(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
    ) -> Ordering {
        match (
            self.throughput.get(&left.id),
            self.throughput.get(&right.id),
        ) {
            (Some(left), Some(right)) => left
                .milli_tokens_per_second
                .cmp(&right.milli_tokens_per_second),
            _ => Ordering::Equal,
        }
    }

    fn recompute_throughput_baseline(&mut self) {
        let mut measured = self
            .throughput
            .values()
            .filter(|estimate| estimate.samples >= MIN_THROUGHPUT_SAMPLES)
            .map(|estimate| estimate.milli_tokens_per_second)
            .collect::<Vec<_>>();
        measured.sort_unstable();
        self.throughput_baseline = measured.get(measured.len() / 2).copied();
    }

    fn compare_account_age(&self, left: &RuntimeCandidate, right: &RuntimeCandidate) -> Ordering {
        if left.kind != CandidateKind::OAuthAccount || right.kind != CandidateKind::OAuthAccount {
            return Ordering::Equal;
        }
        match (
            self.created_at_ms.get(&left.id),
            self.created_at_ms.get(&right.id),
        ) {
            (Some(left), Some(right)) => right.cmp(left),
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => Ordering::Equal,
        }
    }
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

fn compare_lru(left: Option<u64>, right: Option<u64>) -> Ordering {
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(left), Some(right)) => right.cmp(&left),
    }
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
                now_ms: 100,
            })
            .is_none());
    }

    #[test]
    fn selection_orders_equal_quota_by_lru_priority_weight_then_id() {
        let mut scheduler = PoolScheduler::new();
        let mut low_priority = candidate("priority-low");
        low_priority.priority = 1;
        low_priority.quota = CandidateQuota::Available(100);
        scheduler.upsert(low_priority);
        let mut high_priority = candidate("priority-high");
        high_priority.priority = 2;
        high_priority.quota = CandidateQuota::Available(100);
        scheduler.upsert(high_priority);
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "priority-high"
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
    fn quota_reserve_beats_lru_and_manual_priority() {
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
    fn oauth_requests_rotate_before_manual_priority() {
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
        assert_eq!(first.candidate_id, "low-priority");
        assert!(scheduler.record_success("low-priority", "gpt-5", 30));
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
        assert_eq!(request(&mut stabilizer), "stabilizer-source");
    }

    #[test]
    fn active_and_sequential_requests_spread_by_available_quota() {
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
            "low"
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
    fn low_quota_account_waits_for_its_proportional_turn() {
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
        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(selected.candidate_id, "low");
    }

    #[test]
    fn equal_quota_sequential_requests_follow_configured_weight() {
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

        assert_eq!(counts.get("full"), Some(&40));
        assert_eq!(counts.get("half"), Some(&20));
        assert_eq!(counts.get("quarter"), Some(&10));
    }

    #[test]
    fn sequential_requests_rotate_proportionally_to_quota_headroom() {
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
                assert_eq!(
                    selected.diagnostics.reason,
                    SelectionReason::AdaptiveBalance
                );
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
        assert_eq!(counts.get("full"), Some(&40));
        assert_eq!(counts.get("half"), Some(&20));
        assert_eq!(counts.get("quarter"), Some(&10));
    }

    #[test]
    fn quota_refresh_preserves_historical_rotation_debt() {
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

        for _ in 0..9 {
            let selection = select(&mut scheduler, &HashSet::new()).unwrap();
            assert_eq!(selection.candidate_id, "second");
            assert!(scheduler.reserve(&selection.candidate_id));
            assert!(scheduler.release(&selection.candidate_id));
        }
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "first"
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

        assert_eq!(counts.get("full"), Some(&4));
        assert_eq!(counts.get("half"), Some(&2));
        assert_eq!(counts.get("quarter"), Some(&1));
    }

    #[test]
    fn two_hundred_concurrent_requests_use_every_eligible_account() {
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

        assert_eq!(counts.len(), 4);
        assert!(counts.values().all(|count| *count > 0));
        let total_weight = 6_300_u64 + 5_400 + 5_200 + 5_100;
        for (id, weight) in [
            ("sixty-three", 6_300_u64),
            ("fifty-four", 5_400),
            ("fifty-two", 5_200),
            ("fifty-one", 5_100),
        ] {
            let actual = u64::from(counts[id]) * total_weight;
            let expected = 200 * weight;
            assert!(actual.abs_diff(expected) <= total_weight);
        }
    }

    #[test]
    fn adaptive_routing_gives_faster_accounts_more_work_without_starving_slow_ones() {
        let mut scheduler = PoolScheduler::new();
        for id in ["fast", "slow"] {
            let mut account = oauth_candidate(id);
            account.quota = CandidateQuota::Available(5_000);
            scheduler.upsert(account);
        }
        for now_ms in 1..=3 {
            assert!(scheduler.record_success_with_metrics(
                "fast",
                "gpt-5",
                now_ms,
                Some(100),
                1_000
            ));
            assert!(scheduler.record_success_with_metrics(
                "slow",
                "gpt-5",
                now_ms,
                Some(25),
                1_000
            ));
        }
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "fast"
        );

        let mut counts = BTreeMap::new();
        for _ in 0..100 {
            let selected = select(&mut scheduler, &HashSet::new()).unwrap();
            assert!(scheduler.reserve(&selected.candidate_id));
            *counts.entry(selected.candidate_id.clone()).or_insert(0_u32) += 1;
            assert!(scheduler.release(&selected.candidate_id));
        }

        assert_eq!(counts["fast"], 67);
        assert_eq!(counts["slow"], 33);
    }

    #[test]
    fn oldest_account_routing_prefers_age_but_never_a_busy_or_ineligible_account() {
        let mut scheduler = PoolScheduler::new();
        scheduler.set_routing_strategy(RoutingStrategy::OldestAccount);
        for (id, created_at, quota) in [
            ("oldest", 10, CandidateQuota::Available(100)),
            ("newer", 20, CandidateQuota::Available(10_000)),
            ("disabled-old", 1, CandidateQuota::Available(10_000)),
            ("exhausted-old", 2, CandidateQuota::Exhausted),
        ] {
            let mut account = oauth_candidate(id);
            account.quota = quota;
            account.enabled = id != "disabled-old";
            scheduler.upsert(account);
            assert!(scheduler.set_candidate_created_at(id, created_at));
        }

        let selected = select(&mut scheduler, &HashSet::new()).unwrap();
        assert_eq!(selected.candidate_id, "oldest");
        assert_eq!(selected.diagnostics.reason, SelectionReason::OldestAccount);
        assert!(scheduler.reserve("oldest"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "newer"
        );
        assert!(scheduler.release("oldest"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "oldest"
        );
    }

    #[test]
    fn throughput_ewma_ignores_short_or_invalid_samples_and_is_bounded() {
        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(oauth_candidate("account"));
        assert!(scheduler.record_success_with_metrics("account", "gpt-5", 1, None, 1_000));
        assert!(scheduler.record_success_with_metrics("account", "gpt-5", 2, Some(10), 0));
        assert!(scheduler.record_success_with_metrics("account", "gpt-5", 3, Some(0), 1_000));
        assert!(scheduler.record_success_with_metrics("account", "gpt-5", 4, Some(15), 1_000));
        assert!(scheduler.throughput.is_empty());

        assert!(scheduler.record_success_with_metrics("account", "gpt-5", 5, Some(20), 1_000));
        assert_eq!(
            scheduler.throughput["account"].milli_tokens_per_second,
            20_000
        );
        assert!(scheduler.record_success_with_metrics("account", "gpt-5", 6, Some(20), 1_000));
        assert_eq!(
            scheduler.throughput["account"].milli_tokens_per_second,
            20_000
        );
        assert!(scheduler.record_success_with_metrics("account", "gpt-5", 7, Some(u64::MAX), 1,));
        assert!(
            scheduler.throughput["account"].milli_tokens_per_second
                <= MAX_THROUGHPUT_MILLI_TOKENS_PER_SECOND
        );
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
                now_ms: 1,
            }),
            None,
            "a continuation cannot move to a candidate that did not create the response"
        );
    }

    #[test]
    fn cooldown_expires_and_success_clears_it_and_updates_lru() {
        let mut scheduler = PoolScheduler::new();
        let mut candidate = candidate("candidate");
        candidate.cooldowns.insert("gpt-5".to_string(), 101);
        candidate.cooldowns.insert("*".to_string(), 101);
        scheduler.upsert(candidate);
        assert_eq!(select(&mut scheduler, &HashSet::new()), None);

        assert!(scheduler.record_success("candidate", "GPT-5", 90));
        assert_eq!(
            scheduler.candidate("candidate").unwrap().last_used_at,
            Some(90)
        );
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
    fn runtime_order_uses_scheduler_preference_and_exposes_live_state() {
        let mut scheduler = PoolScheduler::new();
        scheduler.upsert(candidate("first"));
        scheduler.upsert(candidate("second"));

        let initial = scheduler.runtime_order(50);
        assert_eq!(initial[0].candidate_id, "first");
        assert!(initial.iter().all(|candidate| candidate.available));

        assert!(scheduler.reserve_for("first", "gpt-5", 50));
        let loaded = scheduler.runtime_order(50);
        assert_eq!(loaded[0].candidate_id, "second");
        assert_eq!(loaded[1].in_flight, 1);
        assert_eq!(loaded[1].dispatches, 1);

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
                now_ms: 100,
            }),
            Some(200)
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
                now_ms: 0,
            })
            .is_some());
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
            scheduler.effective_weight(scheduler.candidate("protected").unwrap()),
            100
        );
    }
}
