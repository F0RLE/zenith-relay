use super::affinity::AffinityCache;
use super::candidate::{CandidateHealth, CandidateKind, CandidateScope, RuntimeCandidate};
use super::capacity::CandidateQuota;
use crate::WireApi;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

const API_SOURCE_PRIMARY_PRIORITY: i32 = 1_000_000;
const API_SOURCE_RESERVE_PRIORITY: i32 = -1_000_000;
const RESPONSE_AFFINITY_MAX_ENTRIES: usize = 4_096;
const RESPONSE_AFFINITY_TTL_MS: u64 = 24 * 60 * 60 * 1_000;

pub struct SelectionRequest<'a> {
    pub model: &'a str,
    pub allowed_protocols: &'a [WireApi],
    pub scope: &'a CandidateScope,
    pub tried: &'a HashSet<String>,
    pub affinity_key: Option<&'a str>,
    pub now_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Selection {
    pub candidate_id: String,
    pub affinity_hit: bool,
    pub diagnostics: RoutingDiagnostics,
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
    session_affinity: AffinityCache,
    response_affinity: AffinityCache,
    in_flight: BTreeMap<String, u32>,
    dispatches: BTreeMap<String, u64>,
}

impl PoolScheduler {
    pub fn new(max_affinity_entries: usize, affinity_ttl_ms: u64) -> Self {
        Self {
            candidates: BTreeMap::new(),
            session_affinity: AffinityCache::new(max_affinity_entries, affinity_ttl_ms),
            response_affinity: AffinityCache::new(
                RESPONSE_AFFINITY_MAX_ENTRIES,
                RESPONSE_AFFINITY_TTL_MS,
            ),
            in_flight: BTreeMap::new(),
            dispatches: BTreeMap::new(),
        }
    }

    pub fn upsert(&mut self, candidate: RuntimeCandidate) {
        self.candidates.insert(candidate.id.clone(), candidate);
    }

    pub fn remove(&mut self, candidate_id: &str) -> Option<RuntimeCandidate> {
        self.session_affinity.invalidate_candidate(candidate_id);
        self.response_affinity.invalidate_candidate(candidate_id);
        self.in_flight.remove(candidate_id);
        self.dispatches.remove(candidate_id);
        self.candidates.remove(candidate_id)
    }

    pub fn candidate(&self, candidate_id: &str) -> Option<&RuntimeCandidate> {
        self.candidates.get(candidate_id)
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
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        let routing_state_changed =
            candidate.enabled != enabled || candidate.health != health || candidate.quota != quota;
        candidate.enabled = enabled;
        candidate.health = health;
        candidate.quota = quota;
        if routing_state_changed {
            self.dispatches.clear();
        }
        true
    }

    pub fn select(&mut self, request: SelectionRequest<'_>) -> Option<Selection> {
        if let Some(key) = request.affinity_key {
            if let Some(candidate_id) = self
                .response_affinity
                .get(key, request.now_ms)
                .map(str::to_string)
            {
                let eligible = self.candidates.get(&candidate_id).is_some_and(|candidate| {
                    !request.tried.contains(&candidate_id)
                        && candidate.is_eligible(
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
                        self.eligible_count(&request),
                    )?;
                    return Some(Selection {
                        candidate_id,
                        affinity_hit: true,
                        diagnostics,
                    });
                }
                return None;
            }
        }

        if let (Some(key), Some(candidate_id)) = (
            request.affinity_key,
            request
                .affinity_key
                .and_then(|key| self.session_affinity.get(key, request.now_ms))
                .map(str::to_string),
        ) {
            match self.candidates.get(&candidate_id) {
                Some(candidate)
                    if !request.tried.contains(&candidate_id)
                        && candidate.is_eligible(
                            request.model,
                            request.allowed_protocols,
                            request.scope,
                            request.now_ms,
                        ) =>
                {
                    self.session_affinity.refresh(key, request.now_ms);
                    let diagnostics = self.diagnostics(
                        &candidate_id,
                        SelectionReason::SessionAffinity,
                        self.eligible_count(&request),
                    )?;
                    return Some(Selection {
                        candidate_id,
                        affinity_hit: true,
                        diagnostics,
                    });
                }
                Some(candidate)
                    if candidate.is_eligible(
                        request.model,
                        request.allowed_protocols,
                        request.scope,
                        request.now_ms,
                    ) => {}
                _ => {
                    self.session_affinity.invalidate(key);
                }
            }
        }

        let eligible = self
            .candidates
            .values()
            .filter(|candidate| {
                !request.tried.contains(&candidate.id)
                    && candidate.is_eligible(
                        request.model,
                        request.allowed_protocols,
                        request.scope,
                        request.now_ms,
                    )
            })
            .collect::<Vec<_>>();
        let selected = eligible.iter().copied().max_by(|left, right| {
            compare_preference(
                left,
                right,
                self.in_flight.get(&left.id).copied().unwrap_or_default(),
                self.in_flight.get(&right.id).copied().unwrap_or_default(),
                self.dispatches.get(&left.id).copied().unwrap_or_default(),
                self.dispatches.get(&right.id).copied().unwrap_or_default(),
            )
        })?;
        let runner_up = eligible
            .iter()
            .copied()
            .filter(|candidate| candidate.id != selected.id)
            .max_by(|left, right| {
                compare_preference(
                    left,
                    right,
                    self.in_flight.get(&left.id).copied().unwrap_or_default(),
                    self.in_flight.get(&right.id).copied().unwrap_or_default(),
                    self.dispatches.get(&left.id).copied().unwrap_or_default(),
                    self.dispatches.get(&right.id).copied().unwrap_or_default(),
                )
            });
        let reason = runner_up.map_or(SelectionReason::OnlyEligible, |runner_up| {
            selection_reason(
                selected,
                runner_up,
                self.in_flight
                    .get(&selected.id)
                    .copied()
                    .unwrap_or_default(),
                self.in_flight
                    .get(&runner_up.id)
                    .copied()
                    .unwrap_or_default(),
                self.dispatches
                    .get(&selected.id)
                    .copied()
                    .unwrap_or_default(),
                self.dispatches
                    .get(&runner_up.id)
                    .copied()
                    .unwrap_or_default(),
            )
        });
        Some(Selection {
            candidate_id: selected.id.clone(),
            affinity_hit: false,
            diagnostics: self.diagnostics(&selected.id, reason, eligible.len())?,
        })
    }

    pub(crate) fn diagnostics(
        &self,
        candidate_id: &str,
        reason: SelectionReason,
        eligible_candidates: usize,
    ) -> Option<RoutingDiagnostics> {
        let candidate = self.candidates.get(candidate_id)?;
        Some(RoutingDiagnostics {
            reason,
            eligible_candidates: u32::try_from(eligible_candidates).unwrap_or(u32::MAX),
            quota_remaining_basis_points: match candidate.quota {
                CandidateQuota::Available(remaining) => Some(remaining),
                CandidateQuota::Unknown | CandidateQuota::Exhausted | CandidateQuota::Stale => None,
            },
            effective_weight: u64::try_from(effective_weight(candidate)).unwrap_or(u64::MAX),
            in_flight_before: self
                .in_flight
                .get(candidate_id)
                .copied()
                .unwrap_or_default(),
            dispatches_before: self
                .dispatches
                .get(candidate_id)
                .copied()
                .unwrap_or_default(),
        })
    }

    fn eligible_count(&self, request: &SelectionRequest<'_>) -> usize {
        self.candidates
            .values()
            .filter(|candidate| {
                !request.tried.contains(&candidate.id)
                    && candidate.is_eligible(
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

    pub fn bind_affinity(
        &mut self,
        key: impl Into<String>,
        candidate_id: &str,
        now_ms: u64,
    ) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        self.session_affinity.bind(key, candidate_id, now_ms);
        true
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

    pub fn invalidate_affinity(&mut self, key: &str) -> bool {
        self.session_affinity.invalidate(key) || self.response_affinity.invalidate(key)
    }

    pub fn invalidate_candidate_affinity(&mut self, candidate_id: &str) -> usize {
        self.session_affinity.invalidate_candidate(candidate_id)
            + self.response_affinity.invalidate_candidate(candidate_id)
    }

    pub fn clear_affinity(&mut self) {
        self.session_affinity.clear();
        self.response_affinity.clear();
    }

    pub(crate) fn reserve(&mut self, candidate_id: &str) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        let in_flight = self.in_flight.entry(candidate_id.to_string()).or_default();
        *in_flight = in_flight.saturating_add(1);
        let dispatches = self.dispatches.entry(candidate_id.to_string()).or_default();
        *dispatches = dispatches.saturating_add(1);
        true
    }

    pub(crate) fn release(&mut self, candidate_id: &str) -> bool {
        let Some(in_flight) = self.in_flight.get_mut(candidate_id) else {
            return false;
        };
        if *in_flight <= 1 {
            self.in_flight.remove(candidate_id);
        } else {
            *in_flight -= 1;
        }
        true
    }

    pub fn record_success(&mut self, candidate_id: &str, model: &str, now_ms: u64) -> bool {
        let Some(candidate) = self.candidates.get_mut(candidate_id) else {
            return false;
        };
        candidate.cooldowns.retain(|candidate_model, _| {
            candidate_model != "*" && !candidate_model.eq_ignore_ascii_case(model)
        });
        candidate.health = CandidateHealth::Healthy;
        candidate.last_used_at = Some(now_ms);
        candidate.consecutive_failures = 0;
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
}

fn compare_preference(
    left: &RuntimeCandidate,
    right: &RuntimeCandidate,
    left_in_flight: u32,
    right_in_flight: u32,
    left_dispatches: u64,
    right_dispatches: u64,
) -> Ordering {
    routing_tier(left)
        .cmp(&routing_tier(right))
        .then_with(|| compare_weighted_load(left, right, left_in_flight, right_in_flight))
        .then_with(|| candidate_kind_preference(left).cmp(&candidate_kind_preference(right)))
        .then_with(|| compare_weighted_dispatches(left, right, left_dispatches, right_dispatches))
        .then_with(|| left.quota.compare_preference(right.quota))
        .then_with(|| compare_lru(left.last_used_at, right.last_used_at))
        .then_with(|| left.priority.cmp(&right.priority))
        .then_with(|| left.weight.cmp(&right.weight))
        .then_with(|| right.id.cmp(&left.id))
}

fn selection_reason(
    selected: &RuntimeCandidate,
    runner_up: &RuntimeCandidate,
    selected_in_flight: u32,
    runner_up_in_flight: u32,
    selected_dispatches: u64,
    runner_up_dispatches: u64,
) -> SelectionReason {
    if routing_tier(selected) != routing_tier(runner_up) {
        SelectionReason::RoutingTier
    } else if compare_weighted_load(selected, runner_up, selected_in_flight, runner_up_in_flight)
        != Ordering::Equal
    {
        SelectionReason::ParallelLoad
    } else if candidate_kind_preference(selected) != candidate_kind_preference(runner_up) {
        SelectionReason::PoolPolicy
    } else if compare_weighted_dispatches(
        selected,
        runner_up,
        selected_dispatches,
        runner_up_dispatches,
    ) != Ordering::Equal
    {
        SelectionReason::FairRotation
    } else if selected.quota.compare_preference(runner_up.quota) != Ordering::Equal {
        SelectionReason::QuotaHeadroom
    } else if compare_lru(selected.last_used_at, runner_up.last_used_at) != Ordering::Equal {
        SelectionReason::LeastRecentlyUsed
    } else if selected.priority != runner_up.priority {
        SelectionReason::ManualPriority
    } else if selected.weight != runner_up.weight {
        SelectionReason::ManualWeight
    } else {
        SelectionReason::StableTieBreak
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

fn compare_weighted_load(
    left: &RuntimeCandidate,
    right: &RuntimeCandidate,
    left_in_flight: u32,
    right_in_flight: u32,
) -> Ordering {
    (u128::from(right_in_flight) * effective_weight(left))
        .cmp(&(u128::from(left_in_flight) * effective_weight(right)))
}

fn compare_weighted_dispatches(
    left: &RuntimeCandidate,
    right: &RuntimeCandidate,
    left_dispatches: u64,
    right_dispatches: u64,
) -> Ordering {
    (u128::from(right_dispatches) * effective_weight(left))
        .cmp(&(u128::from(left_dispatches) * effective_weight(right)))
}

fn effective_weight(candidate: &RuntimeCandidate) -> u128 {
    u128::from(candidate.weight.max(1)) * u128::from(candidate.quota.routing_weight_factor())
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
            affinity_key: None,
            now_ms: 100,
        })
    }

    #[test]
    fn availability_updates_take_effect_while_candidate_is_in_flight() {
        let mut scheduler = PoolScheduler::new(8, 60_000);
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

        let mut scheduler = PoolScheduler::new(1, 100);
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
                affinity_key: None,
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
                affinity_key: None,
                now_ms: 100,
            })
            .is_none());
    }

    #[test]
    fn selection_orders_equal_quota_by_lru_priority_weight_then_id() {
        let mut scheduler = PoolScheduler::new(1, 100);
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

        scheduler = PoolScheduler::new(1, 100);
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

        scheduler = PoolScheduler::new(1, 100);
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

        scheduler = PoolScheduler::new(1, 100);
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

        scheduler = PoolScheduler::new(1, 100);
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
        let mut scheduler = PoolScheduler::new(1, 100);
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
        let mut scheduler = PoolScheduler::new(1, 100);
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
                    affinity_key: None,
                    now_ms: 100,
                })
                .unwrap()
                .candidate_id
        };

        let mut primary = PoolScheduler::new(1, 100);
        let mut source = candidate("primary-source");
        source.priority = API_SOURCE_PRIMARY_PRIORITY;
        primary.upsert(source);
        primary.upsert(oauth_candidate("account"));
        assert_eq!(request(&mut primary), "primary-source");

        let mut reserve = PoolScheduler::new(1, 100);
        let mut source = candidate("reserve-source");
        source.priority = API_SOURCE_RESERVE_PRIORITY;
        reserve.upsert(source);
        reserve.upsert(oauth_candidate("account"));
        assert_eq!(request(&mut reserve), "account");

        let mut stabilizer = PoolScheduler::new(1, 100);
        stabilizer.upsert(candidate("stabilizer-source"));
        stabilizer.upsert(oauth_candidate("account"));
        assert_eq!(request(&mut stabilizer), "account");
        assert!(stabilizer.reserve("account"));
        assert_eq!(request(&mut stabilizer), "stabilizer-source");
    }

    #[test]
    fn active_and_sequential_requests_spread_by_available_quota() {
        let mut scheduler = PoolScheduler::new(1, 100);
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
            "low"
        );
        assert!(scheduler.reserve("low"));
        assert!(scheduler.release("low"));
        assert_eq!(
            select(&mut scheduler, &HashSet::new())
                .unwrap()
                .candidate_id,
            "full"
        );
    }

    #[test]
    fn equal_quota_sequential_requests_follow_configured_weight() {
        let mut scheduler = PoolScheduler::new(1, 100);
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
        let mut scheduler = PoolScheduler::new(1, 100);
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
        assert_eq!(counts.get("full"), Some(&40));
        assert_eq!(counts.get("half"), Some(&20));
        assert_eq!(counts.get("quarter"), Some(&10));
    }

    #[test]
    fn quota_refresh_rebases_historical_rotation_debt() {
        let mut scheduler = PoolScheduler::new(1, 100);
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

        let mut selected = BTreeSet::new();
        for _ in 0..2 {
            let selection = select(&mut scheduler, &HashSet::new()).unwrap();
            assert!(scheduler.reserve(&selection.candidate_id));
            assert!(scheduler.release(&selection.candidate_id));
            selected.insert(selection.candidate_id);
        }
        assert_eq!(selected, ["first".to_string(), "second".to_string()].into());
    }

    #[test]
    fn concurrent_requests_follow_available_quota_headroom() {
        let mut scheduler = PoolScheduler::new(1, 100);
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
        let mut scheduler = PoolScheduler::new(1, 100);
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
    fn affinity_wins_but_never_bypasses_filters_or_tried_exclusion() {
        let mut scheduler = PoolScheduler::new(4, 100);
        let mut preferred = candidate("preferred");
        preferred.priority = 10;
        scheduler.upsert(preferred);
        scheduler.upsert(candidate("affinity"));
        assert!(scheduler.bind_affinity("session", "affinity", 0));

        let scope = CandidateScope::default();
        let empty = HashSet::new();
        let selection = scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &empty,
                affinity_key: Some("session"),
                now_ms: 1,
            })
            .unwrap();
        assert_eq!(selection.candidate_id, "affinity");
        assert!(selection.affinity_hit);
        assert_eq!(
            selection.diagnostics.reason,
            SelectionReason::SessionAffinity
        );
        assert_eq!(selection.diagnostics.eligible_candidates, 2);

        let tried: HashSet<String> = ["affinity".to_string()].into();
        assert_eq!(
            scheduler
                .select(SelectionRequest {
                    model: "gpt-5",
                    allowed_protocols: &[WireApi::Responses],
                    scope: &scope,
                    tried: &tried,
                    affinity_key: Some("session"),
                    now_ms: 1,
                })
                .unwrap()
                .candidate_id,
            "preferred"
        );
        scheduler.set_cooldown("affinity", "gpt-5", 10);
        assert_eq!(
            scheduler
                .select(SelectionRequest {
                    model: "gpt-5",
                    allowed_protocols: &[WireApi::Responses],
                    scope: &scope,
                    tried: &empty,
                    affinity_key: Some("session"),
                    now_ms: 1,
                })
                .unwrap()
                .candidate_id,
            "preferred"
        );
    }

    #[test]
    fn response_affinity_is_mandatory_when_session_affinity_is_disabled() {
        let mut scheduler = PoolScheduler::new(0, 0);
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
                affinity_key: Some("response"),
                now_ms: 1,
            })
            .unwrap();
        assert_eq!(selection.candidate_id, "creator");
        assert!(selection.affinity_hit);
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
                affinity_key: Some("response"),
                now_ms: 1,
            }),
            None,
            "a continuation cannot move to a candidate that did not create the response"
        );
    }

    #[test]
    fn cooldown_expires_and_success_clears_it_and_updates_lru() {
        let mut scheduler = PoolScheduler::new(1, 100);
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
                affinity_key: None,
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
    fn earliest_retry_ignores_candidates_blocked_for_non_cooldown_reasons() {
        let mut scheduler = PoolScheduler::new(1, 100);
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
                affinity_key: None,
                now_ms: 100,
            }),
            Some(200)
        );
    }

    #[test]
    fn account_scope_allows_oauth_ready_candidate_shape() {
        let mut scheduler = PoolScheduler::new(1, 100);
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
                affinity_key: None,
                now_ms: 0,
            })
            .is_some());
    }

    #[test]
    fn translated_protocols_share_the_same_scheduler() {
        let mut scheduler = PoolScheduler::new(1, 100);
        let mut candidate = candidate("chat-source");
        candidate.protocol = WireApi::ChatCompletions;
        scheduler.upsert(candidate);

        assert!(scheduler
            .select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses, WireApi::ChatCompletions],
                scope: &CandidateScope::default(),
                tried: &HashSet::new(),
                affinity_key: None,
                now_ms: 0,
            })
            .is_some());
    }

    #[test]
    fn explicit_empty_scope_selects_no_candidates() {
        let mut scheduler = PoolScheduler::new(1, 100);
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
                affinity_key: None,
                now_ms: 0,
            })
            .is_none());
    }
}
