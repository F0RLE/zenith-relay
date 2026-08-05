use super::{InFlightLane, PoolScheduler, SelectionReason};
use crate::scheduler::{CandidateKind, CandidateQuota, RuntimeCandidate};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

pub(super) const API_SOURCE_PRIMARY_PRIORITY: i32 = 1_000_000;
pub(super) const API_SOURCE_RESERVE_PRIORITY: i32 = -1_000_000;
const QUOTA_NOISE_TIE_BPS: u64 = 1;
const QUOTA_RESET_TIE_BPS: u64 = 100;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingStrategy {
    #[default]
    Adaptive,
    QuotaHighest,
    SubscriptionExpiry,
    SubscriptionPlan,
}

impl PoolScheduler {
    pub(super) fn compare_preference(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
        lane: InFlightLane,
    ) -> Ordering {
        let left_in_flight = self.in_flight_count(&left.id, lane);
        let right_in_flight = self.in_flight_count(&right.id, lane);
        let left_dispatches = self.rotation_dispatch_count(left, lane);
        let right_dispatches = self.rotation_dispatch_count(right, lane);
        let common = routing_tier(left)
            .cmp(&routing_tier(right))
            .then_with(|| candidate_kind_preference(left).cmp(&candidate_kind_preference(right)));
        // Parallel-load balancing protects OAuth accounts from being selected by
        // every concurrent chat. API sources are connection-based providers:
        // an active request must not make a different API source win selection.
        // They still honor explicit routing tier, quota, configured weight, and
        // normal fallback rules below.
        let load = || {
            if left.kind == CandidateKind::OAuthAccount && right.kind == CandidateKind::OAuthAccount
            {
                right_in_flight.cmp(&left_in_flight)
            } else {
                Ordering::Equal
            }
        };
        let fair_rotation =
            || self.compare_equal_quota_rotation(left, right, left_dispatches, right_dispatches);

        match self.routing_strategy {
            RoutingStrategy::Adaptive => common
                .then_with(|| self.compare_quota_and_reset(left, right))
                .then_with(load)
                .then_with(fair_rotation)
                .then_with(|| right.id.cmp(&left.id)),
            RoutingStrategy::QuotaHighest => common
                .then_with(|| self.compare_quota_and_reset(left, right))
                .then_with(load)
                .then_with(|| right.id.cmp(&left.id)),
            RoutingStrategy::SubscriptionExpiry => common
                .then_with(|| self.compare_subscription_expiry(left, right))
                .then_with(|| self.compare_quota_and_reset(left, right))
                .then_with(load)
                .then_with(fair_rotation)
                .then_with(|| right.id.cmp(&left.id)),
            RoutingStrategy::SubscriptionPlan => common
                .then_with(|| self.compare_subscription_plan(left, right))
                .then_with(|| self.compare_quota_and_reset(left, right))
                .then_with(load)
                .then_with(fair_rotation)
                .then_with(|| right.id.cmp(&left.id)),
        }
    }

    pub(super) fn selection_reason(
        &self,
        selected: &RuntimeCandidate,
        runner_up: &RuntimeCandidate,
        lane: InFlightLane,
    ) -> SelectionReason {
        let selected_in_flight = self.in_flight_count(&selected.id, lane);
        let runner_up_in_flight = self.in_flight_count(&runner_up.id, lane);
        let selected_dispatches = self.rotation_dispatch_count(selected, lane);
        let runner_up_dispatches = self.rotation_dispatch_count(runner_up, lane);
        if routing_tier(selected) != routing_tier(runner_up)
            || candidate_kind_preference(selected) != candidate_kind_preference(runner_up)
        {
            SelectionReason::SourceRole
        } else if self.routing_strategy == RoutingStrategy::SubscriptionExpiry
            && self.compare_subscription_expiry(selected, runner_up) != Ordering::Equal
        {
            SelectionReason::SubscriptionExpiry
        } else if self.routing_strategy == RoutingStrategy::SubscriptionPlan
            && self.compare_subscription_plan(selected, runner_up) != Ordering::Equal
        {
            SelectionReason::SubscriptionPlan
        } else if self.compare_quota_and_reset(selected, runner_up) != Ordering::Equal {
            SelectionReason::QuotaHeadroom
        } else if selected.kind == CandidateKind::OAuthAccount
            && runner_up.kind == CandidateKind::OAuthAccount
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
            if selected.kind == CandidateKind::ApiSource
                && runner_up.kind == CandidateKind::ApiSource
            {
                SelectionReason::WeightedRotation
            } else {
                SelectionReason::FairRotation
            }
        } else {
            SelectionReason::StableTieBreak
        }
    }

    fn compare_quota_and_reset(
        &self,
        left: &RuntimeCandidate,
        right: &RuntimeCandidate,
    ) -> Ordering {
        match (self.routing_quota(left), self.routing_quota(right)) {
            (CandidateQuota::Available(left_quota), CandidateQuota::Available(right_quota))
                if left_quota.abs_diff(right_quota) <= QUOTA_NOISE_TIE_BPS =>
            {
                Ordering::Equal
            }
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

    pub(super) fn routing_quota_factor(&self, candidate: &RuntimeCandidate) -> u64 {
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

    pub(super) fn routing_quota(&self, candidate: &RuntimeCandidate) -> CandidateQuota {
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
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
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

pub(super) fn routing_tier(candidate: &RuntimeCandidate) -> i8 {
    match candidate.kind {
        CandidateKind::OAuthAccount => 0,
        CandidateKind::ApiSource if candidate.priority >= API_SOURCE_PRIMARY_PRIORITY => 1,
        CandidateKind::ApiSource if candidate.priority <= API_SOURCE_RESERVE_PRIORITY => -1,
        CandidateKind::ApiSource => 0,
    }
}

pub(super) fn candidate_kind_preference(candidate: &RuntimeCandidate) -> u8 {
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
