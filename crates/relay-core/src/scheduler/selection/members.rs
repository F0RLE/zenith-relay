//! Physical member identity, saved policy and protocol-route preference.

use super::*;
use crate::{PoolMemberKind, PoolRoutingMember, PoolRoutingPolicy};
use std::cmp::Ordering;

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
    pub(crate) fn member_key_for(&self, candidate_id: &str) -> Option<String> {
        self.candidates.get(candidate_id).map(member_key)
    }

    pub(crate) fn active_member_keys(&self, now_ms: u64, recent_ms: u64) -> BTreeSet<String> {
        self.candidates
            .values()
            .filter(|candidate| {
                self.activity
                    .in_flight_count(&candidate.id, InFlightLane::Text)
                    > 0
                    || self
                        .activity
                        .in_flight_count(&candidate.id, InFlightLane::Image)
                        > 0
                    || candidate
                        .last_used_at
                        .is_some_and(|at| at <= now_ms && now_ms.saturating_sub(at) < recent_ms)
            })
            .map(member_key)
            .collect()
    }

    pub(crate) fn routes_for_members(&self, members: &BTreeSet<String>) -> HashSet<String> {
        self.candidates
            .values()
            .filter(|candidate| members.contains(&member_key(candidate)))
            .map(|candidate| candidate.id.clone())
            .collect()
    }

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
            .validate_activation()
            .map_err(|message| crate::Error::Validation(message.into()))?;
        if self.pool_routing.as_ref() != Some(&policy) {
            self.pool_routing = Some(policy);
            self.sync_rotation_mode();
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
        crate::resolve_pool_routing(Some(&PoolRoutingPolicy::default()), members)
    }

    pub(super) fn member_policy(
        &self,
        candidate: &RuntimeCandidate,
    ) -> Option<(usize, &PoolRoutingMember)> {
        let (kind, id) = identity(candidate);
        self.pool_routing
            .as_ref()?
            .members
            .iter()
            .enumerate()
            .find(|(_, member)| member.kind == kind && member.id == id)
    }

    pub(super) fn member_weight(&self, candidate: &RuntimeCandidate) -> u32 {
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
}
