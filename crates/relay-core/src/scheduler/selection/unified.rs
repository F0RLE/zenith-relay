use super::*;
use crate::{PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy};
use std::cmp::Ordering;

const SMART_SCORE_SLACK: u32 = 750;

pub(super) fn member_key(candidate: &RuntimeCandidate) -> String {
    match candidate.kind {
        CandidateKind::OAuthAccount => format!(
            "account:{}",
            candidate.account_id.as_deref().unwrap_or(&candidate.id)
        ),
        CandidateKind::ApiSource => format!("source:{}", candidate.source_id),
    }
}

fn identity(candidate: &RuntimeCandidate) -> (PoolMemberKind, &str) {
    match candidate.kind {
        CandidateKind::OAuthAccount => (
            PoolMemberKind::Account,
            candidate.account_id.as_deref().unwrap_or(&candidate.id),
        ),
        CandidateKind::ApiSource => (PoolMemberKind::Source, &candidate.source_id),
    }
}

impl PoolScheduler {
    pub(crate) fn set_native_route(&mut self, candidate_id: &str, native: bool) {
        if native {
            self.native_routes.insert(candidate_id.to_string());
        } else {
            self.native_routes.remove(candidate_id);
        }
    }

    pub(super) fn compare_member_routes(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
    ) -> Ordering {
        if member_key(left) == member_key(right) {
            self.native_routes
                .contains(&left.id)
                .cmp(&self.native_routes.contains(&right.id))
        } else {
            Ordering::Equal
        }
    }
    pub fn set_pool_routing(&mut self, policy: PoolRoutingPolicy) -> crate::Result<()> {
        policy
            .validate()
            .map_err(|message| crate::Error::Validation(message.into()))?;
        if self.pool_routing.as_ref() != Some(&policy) {
            self.rotation_credit.clear();
            self.pool_routing = Some(policy);
        }
        Ok(())
    }

    /// Hosts supply the complete configured inventory, including unavailable
    /// members. Direct core callers migrate their candidate inventory here.
    pub fn migrated_pool_routing(&self) -> PoolRoutingPolicy {
        let mut seen = BTreeSet::new();
        let members = self
            .candidates
            .values()
            .filter_map(|candidate| {
                let (kind, id) = identity(candidate);
                seen.insert((kind, id))
                    .then(|| (kind, id.to_owned(), candidate.priority, candidate.weight))
            })
            .collect();
        crate::resolve_pool_routing(None, members)
    }

    fn member_policy(&self, candidate: &RuntimeCandidate) -> Option<(usize, &PoolRoutingMember)> {
        let (kind, id) = identity(candidate);
        self.pool_routing
            .as_ref()?
            .members
            .iter()
            .enumerate()
            .find(|(_, member)| member.kind == kind && member.id == id)
    }

    fn member_rank(&self, candidate: &RuntimeCandidate) -> usize {
        self.member_policy(candidate)
            .map_or(usize::MAX, |(index, _)| index)
    }

    fn member_weight(&self, candidate: &RuntimeCandidate) -> u32 {
        self.member_policy(candidate)
            .map_or(candidate.weight.clamp(1, 100), |(_, member)| member.weight)
    }

    pub(super) fn member_capacity_allows(&self, candidate: &RuntimeCandidate) -> bool {
        self.member_policy(candidate).is_none_or(|(_, member)| {
            member.max_concurrency == 0
                || self
                    .member_activity
                    .active_request_count(&member_key(candidate))
                    < member.max_concurrency
        })
    }

    fn smart_score(&self, candidate: &RuntimeCandidate, lane: InFlightLane, now_ms: u64) -> u32 {
        let load = self
            .member_activity
            .in_flight_count(&member_key(candidate), lane);
        let load_score = 5_000 / (1 + load);
        // Unknown/stale quota is neutral; it is not evidence of exhaustion.
        let quota = match self.routing_quota(candidate) {
            CandidateQuota::Available(_)
                if candidate.quota_updated_at_ms.is_some_and(|updated_at_ms| {
                    now_ms.saturating_sub(updated_at_ms) > self.quota_stale_after_ms
                }) =>
            {
                CandidateQuota::Stale
            }
            quota => quota,
        };
        let quota_score = match quota {
            CandidateQuota::Available(value) => value.min(10_000) as u32 / 4,
            CandidateQuota::Unknown | CandidateQuota::Stale => 1_250,
            CandidateQuota::Exhausted => 0,
        };
        let penalty = self.recent_failure_penalty(candidate, now_ms);
        (load_score + quota_score).saturating_sub(penalty)
    }

    pub(super) fn compare_unified_preference(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
        lane: InFlightLane,
        now_ms: u64,
    ) -> Ordering {
        let mode = self
            .pool_routing
            .as_ref()
            .map(|p| p.mode)
            .unwrap_or_default();
        let rank = || self.member_rank(right).cmp(&self.member_rank(left));
        let primary = match mode {
            PoolRoutingMode::InOrder => rank(),
            PoolRoutingMode::Smart => self
                .smart_score(left, lane, now_ms)
                .cmp(&self.smart_score(right, lane, now_ms)),
            PoolRoutingMode::RoundRobin => self
                .rotation_score(left, lane, self.member_weight(left))
                .cmp(&self.rotation_score(right, lane, self.member_weight(right)))
                .then_with(rank),
        };
        self.compare_member_routes(left, right)
            .then(primary)
            .then_with(|| right.id.cmp(&left.id))
    }

    fn rotation_score(&self, candidate: &RuntimeCandidate, lane: InFlightLane, weight: u32) -> i64 {
        self.rotation_credit
            .get(&(member_key(candidate), lane))
            .copied()
            .unwrap_or_default()
            + i64::from(weight)
    }

    pub(super) fn select_baseline<'a>(
        &self,
        eligible: &[&'a RuntimeCandidate],
        lane: InFlightLane,
        now_ms: u64,
    ) -> Option<(&'a RuntimeCandidate, Vec<(String, u32)>)> {
        let Some(policy) = &self.pool_routing else {
            return eligible
                .iter()
                .copied()
                .max_by(|a, b| self.compare_preference(a, b, lane, now_ms))
                .map(|candidate| (candidate, Vec::new()));
        };
        let mut by_member = BTreeMap::new();
        for &candidate in eligible {
            let entry = by_member.entry(member_key(candidate)).or_insert(candidate);
            if self
                .compare_unified_preference(candidate, entry, lane, now_ms)
                .is_gt()
            {
                *entry = candidate;
            }
        }
        let mut ordered = by_member.into_values().collect::<Vec<_>>();
        ordered.sort_by(|a, b| self.compare_unified_preference(b, a, lane, now_ms));
        let baseline = *ordered.first()?;
        if policy.mode == PoolRoutingMode::InOrder {
            return Some((baseline, Vec::new()));
        }
        if policy.mode == PoolRoutingMode::Smart {
            let floor = self
                .smart_score(baseline, lane, now_ms)
                .saturating_sub(SMART_SCORE_SLACK);
            ordered.retain(|candidate| self.smart_score(candidate, lane, now_ms) >= floor);
        }
        let weight = |candidate: &RuntimeCandidate| {
            let weight = self.member_weight(candidate);
            if policy.mode == PoolRoutingMode::Smart {
                weight * (1_000 + self.smart_score(candidate, lane, now_ms) / 10)
            } else {
                weight
            }
        };
        let selected = ordered.iter().copied().max_by(|a, b| {
            self.rotation_score(a, lane, weight(a))
                .cmp(&self.rotation_score(b, lane, weight(b)))
                .then_with(|| self.compare_unified_preference(a, b, lane, now_ms))
        })?;
        let members = ordered
            .iter()
            .map(|candidate| (member_key(candidate), weight(candidate)))
            .collect();
        Some((selected, members))
    }

    /// Commit smooth weighted rotation only after a slot was reserved. Preview,
    /// rejected reservations, and failed validation never advance the cycle.
    pub(crate) fn commit_rotation(&mut self, selection: &Selection, image: bool) {
        let Some(candidate) = self.candidates.get(&selection.candidate_id) else {
            return;
        };
        let selected = member_key(candidate);
        if !selection
            .rotation_members
            .iter()
            .any(|(id, _)| *id == selected)
        {
            return;
        }
        let lane = if image {
            InFlightLane::Image
        } else {
            InFlightLane::Text
        };
        let total: i64 = selection
            .rotation_members
            .iter()
            .map(|(_, weight)| i64::from(*weight))
            .sum();
        for (id, weight) in &selection.rotation_members {
            let credit = self.rotation_credit.entry((id.clone(), lane)).or_default();
            *credit = (*credit + i64::from(*weight) - if *id == selected { total } else { 0 })
                .clamp(-total * 2, total * 2);
        }
    }

    pub(super) fn unified_prompt_affinity_allows(
        &self,
        preferred: &RuntimeCandidate,
        eligible: &[&RuntimeCandidate],
        lane: InFlightLane,
        now_ms: u64,
    ) -> bool {
        if self
            .pool_routing
            .as_ref()
            .is_none_or(|policy| policy.mode != PoolRoutingMode::Smart)
        {
            return false;
        }
        // The weighted winner may sit at the bottom of the near-best group.
        // Applying the slack to it again would let affinity escape that group.
        let best_score = eligible
            .iter()
            .map(|candidate| self.smart_score(candidate, lane, now_ms))
            .max()
            .unwrap_or_default();
        self.smart_score(preferred, lane, now_ms)
            .saturating_add(SMART_SCORE_SLACK)
            >= best_score
            && self.member_capacity_allows(preferred)
    }
}
