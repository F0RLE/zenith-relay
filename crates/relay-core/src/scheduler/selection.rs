use super::affinity::AffinityCache;
use super::candidate::{CandidateHealth, CandidateScope, RuntimeCandidate};
use crate::WireApi;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};

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
}

#[derive(Clone, Debug)]
pub struct PoolScheduler {
    candidates: BTreeMap<String, RuntimeCandidate>,
    affinity: AffinityCache,
    in_flight: BTreeMap<String, u32>,
}

impl PoolScheduler {
    pub fn new(max_affinity_entries: usize, affinity_ttl_ms: u64) -> Self {
        Self {
            candidates: BTreeMap::new(),
            affinity: AffinityCache::new(max_affinity_entries, affinity_ttl_ms),
            in_flight: BTreeMap::new(),
        }
    }

    pub fn upsert(&mut self, candidate: RuntimeCandidate) {
        self.candidates.insert(candidate.id.clone(), candidate);
    }

    pub fn remove(&mut self, candidate_id: &str) -> Option<RuntimeCandidate> {
        self.affinity.invalidate_candidate(candidate_id);
        self.in_flight.remove(candidate_id);
        self.candidates.remove(candidate_id)
    }

    pub fn candidate(&self, candidate_id: &str) -> Option<&RuntimeCandidate> {
        self.candidates.get(candidate_id)
    }

    pub fn candidates(&self) -> impl Iterator<Item = &RuntimeCandidate> {
        self.candidates.values()
    }

    pub fn select(&mut self, request: SelectionRequest<'_>) -> Option<Selection> {
        if let (Some(key), Some(candidate_id)) = (
            request.affinity_key,
            request
                .affinity_key
                .and_then(|key| self.affinity.get(key, request.now_ms))
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
                    self.affinity.refresh(key, request.now_ms);
                    return Some(Selection {
                        candidate_id,
                        affinity_hit: true,
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
                    self.affinity.invalidate(key);
                }
            }
        }

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
            .max_by(|left, right| {
                compare_preference(
                    left,
                    right,
                    self.in_flight.get(&left.id).copied().unwrap_or_default(),
                    self.in_flight.get(&right.id).copied().unwrap_or_default(),
                )
            })
            .map(|candidate| Selection {
                candidate_id: candidate.id.clone(),
                affinity_hit: false,
            })
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
        self.affinity.bind(key, candidate_id, now_ms);
        true
    }

    pub fn invalidate_affinity(&mut self, key: &str) -> bool {
        self.affinity.invalidate(key)
    }

    pub fn invalidate_candidate_affinity(&mut self, candidate_id: &str) -> usize {
        self.affinity.invalidate_candidate(candidate_id)
    }

    pub fn clear_affinity(&mut self) {
        self.affinity.clear();
    }

    pub(crate) fn reserve(&mut self, candidate_id: &str) -> bool {
        if !self.candidates.contains_key(candidate_id) {
            return false;
        }
        let in_flight = self.in_flight.entry(candidate_id.to_string()).or_default();
        *in_flight = in_flight.saturating_add(1);
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
) -> Ordering {
    left.priority
        .cmp(&right.priority)
        .then_with(|| compare_weighted_load(left, right, left_in_flight, right_in_flight))
        .then_with(|| left.quota.compare_preference(right.quota))
        .then_with(|| compare_lru(left.last_used_at, right.last_used_at))
        .then_with(|| left.weight.cmp(&right.weight))
        .then_with(|| right.id.cmp(&left.id))
}

fn compare_weighted_load(
    left: &RuntimeCandidate,
    right: &RuntimeCandidate,
    left_in_flight: u32,
    right_in_flight: u32,
) -> Ordering {
    (u128::from(right_in_flight) * u128::from(left.weight))
        .cmp(&(u128::from(left_in_flight) * u128::from(right.weight)))
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
    fn selection_orders_priority_quota_lru_weight_then_id() {
        let mut scheduler = PoolScheduler::new(1, 100);
        let mut low_priority = candidate("priority-low");
        low_priority.priority = 1;
        low_priority.quota = CandidateQuota::Available(100);
        scheduler.upsert(low_priority);
        let mut high_priority = candidate("priority-high");
        high_priority.priority = 2;
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
    fn active_requests_spread_before_quota_and_release_restores_preference() {
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
            "full"
        );
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
        assert_eq!(
            scheduler.select(SelectionRequest {
                model: "gpt-5",
                allowed_protocols: &[WireApi::Responses],
                scope: &scope,
                tried: &empty,
                affinity_key: Some("session"),
                now_ms: 1,
            }),
            Some(Selection {
                candidate_id: "affinity".to_string(),
                affinity_hit: true,
            })
        );

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
