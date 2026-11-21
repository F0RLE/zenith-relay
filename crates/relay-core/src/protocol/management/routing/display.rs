use super::super::{AccountSummary, OperationalStatus, RefreshStatus, SourceSummary};
use crate::{
    CandidateKind, CandidateRuntimeSnapshot, PoolMemberKind, PoolRoutingPolicy, RotationPreset,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Model-independent presentation. Exact next admission remains scoped to
/// request routes and affinity, represented separately by runtime snapshots.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PoolRotationView {
    #[serde(default)]
    pub inventory_revision: u64,
    pub preset: Option<RotationPreset>,
    pub member_order: Vec<String>,
    #[serde(default)]
    pub member_statuses: BTreeMap<String, OperationalStatus>,
}

#[derive(Clone, Eq, PartialEq)]
struct DisplayMember {
    kind: PoolMemberKind,
    id: String,
    status: OperationalStatus,
    runtime_recoverable: bool,
    weekly_exhausted_until: Option<u64>,
}

/// Compact inventory captured by a full snapshot. Activity refreshes reuse it
/// without reading credentials, catalogs, usage ledgers or account metadata.
#[derive(Clone, Eq, PartialEq)]
pub struct PoolRotationProjection {
    policy: PoolRoutingPolicy,
    members: Vec<DisplayMember>,
}

pub fn pool_rotation_view(
    policy: &PoolRoutingPolicy,
    sources: &[SourceSummary],
    accounts: &[AccountSummary],
    runtime_order: &[CandidateRuntimeSnapshot],
    now_ms: u64,
) -> PoolRotationView {
    PoolRotationProjection::new(policy, sources, accounts, now_ms).view(runtime_order, now_ms)
}

impl PoolRotationProjection {
    pub fn new(
        policy: &PoolRoutingPolicy,
        sources: &[SourceSummary],
        accounts: &[AccountSummary],
        now_ms: u64,
    ) -> Self {
        let accounts: BTreeMap<_, _> = accounts
            .iter()
            .map(|member| (member.id.as_str(), member))
            .collect();
        let sources: BTreeMap<_, _> = sources
            .iter()
            .map(|member| (member.id.as_str(), member))
            .collect();
        let members = policy
            .members
            .iter()
            .filter_map(|member| {
                let (status, runtime_recoverable, weekly_exhausted_until) = match member.kind {
                    PoolMemberKind::Account => {
                        let account = accounts
                            .get(member.id.as_str())
                            .filter(|account| account.in_pool && !account.route_inactive)?;
                        let exhausted_until = account
                            .quota
                            .secondary
                            .as_ref()
                            .filter(|window| {
                                account.refresh_state.quota == RefreshStatus::Fresh
                                    && account.quota.error.is_none()
                                    && window.available_basis_points == Some(0)
                                    && crate::quota::observation_is_fresh(
                                        Some(window.observed_at_ms),
                                        now_ms,
                                        crate::QUOTA_STALE_AFTER_MS,
                                    )
                                    && window.reset_at_ms.is_none_or(|reset| now_ms < reset)
                            })
                            .map(|window| {
                                window
                                    .observed_at_ms
                                    .saturating_add(crate::QUOTA_STALE_AFTER_MS)
                                    .min(
                                        window
                                            .reset_at_ms
                                            .map_or(u64::MAX, |reset| reset.saturating_sub(1)),
                                    )
                            });
                        (
                            account.operational_status,
                            account.enabled
                                && !account.draining
                                && account.secret_available
                                && account.proxy_available
                                && !account.recovery_required
                                && account.operational_status != OperationalStatus::Disabled
                                && account.routing_block_reason.is_none()
                                && account.last_error_code.as_deref()
                                    != Some("siwc_plan_use_disabled"),
                            exhausted_until,
                        )
                    }
                    PoolMemberKind::Source => {
                        let source = sources
                            .get(member.id.as_str())
                            .filter(|source| source.in_pool)?;
                        (
                            source.operational_status,
                            source.enabled
                                && !source.draining
                                && source.secret_available
                                && source.operational_status != OperationalStatus::Disabled,
                            None,
                        )
                    }
                };
                Some(DisplayMember {
                    kind: member.kind,
                    id: member.id.clone(),
                    status,
                    runtime_recoverable,
                    weekly_exhausted_until,
                })
            })
            .collect();
        Self {
            policy: policy.clone(),
            members,
        }
    }

    pub fn view(
        &self,
        runtime_order: &[CandidateRuntimeSnapshot],
        now_ms: u64,
    ) -> PoolRotationView {
        let sources: std::collections::BTreeSet<_> = self
            .members
            .iter()
            .filter(|member| member.kind == PoolMemberKind::Source)
            .map(|member| member.id.as_str())
            .collect();
        let mut runtime = BTreeMap::<String, Vec<(usize, &CandidateRuntimeSnapshot)>>::new();
        for (rank, candidate) in runtime_order.iter().enumerate() {
            let key = match candidate.kind {
                CandidateKind::OAuthAccount => {
                    crate::scheduler::account_member_key(&candidate.candidate_id)
                }
                CandidateKind::ApiSource => {
                    let id = candidate.candidate_id.as_str();
                    let id = if sources.contains(id) {
                        id
                    } else {
                        id.rsplit_once("::").map_or(id, |(source, _)| source)
                    };
                    crate::scheduler::source_member_key(id)
                }
            };
            runtime.entry(key).or_default().push((rank, candidate));
        }
        let mut rows = Vec::new();
        let mut member_statuses = BTreeMap::new();
        for (saved_rank, member) in self.members.iter().enumerate() {
            let key = match member.kind {
                PoolMemberKind::Account => crate::scheduler::account_member_key(&member.id),
                PoolMemberKind::Source => crate::scheduler::source_member_key(&member.id),
            };
            let candidates = runtime.get(&key).map_or(&[][..], Vec::as_slice);
            let active = candidates
                .iter()
                .any(|(_, candidate)| candidate.active_request_count > 0);
            let available = candidates.iter().any(|(_, candidate)| candidate.available);
            let capacity_wait = candidates
                .iter()
                .any(|(_, candidate)| candidate.capacity_wait);
            let waiting = !available
                && candidates.iter().any(|(_, candidate)| {
                    candidate.quota_wait
                        || candidate.capacity_wait
                        || candidate.next_retry_at_ms.is_some()
                });
            let status = if member.runtime_recoverable && !candidates.is_empty() {
                OperationalStatus::Rotation.with_runtime_available(
                    // A full lane prevents new admission, not membership in
                    // rotation. The runtime snapshot retains the busy state.
                    Some(available || capacity_wait),
                    candidates.iter().any(|(_, candidate)| candidate.quota_wait),
                )
            } else {
                member.status
            };
            member_statuses.insert(key.clone(), status);
            let status_rank = match status {
                OperationalStatus::Rotation if waiting => 1,
                OperationalStatus::Rotation => 0,
                OperationalStatus::QuotaWait if available => 0,
                OperationalStatus::QuotaWait => 1,
                OperationalStatus::Unavailable if waiting => 1,
                OperationalStatus::Unavailable => 2,
                OperationalStatus::Disabled => 3,
            };
            let runtime_rank = candidates.first().map_or(usize::MAX, |(rank, _)| *rank);
            let retry = candidates
                .iter()
                .filter_map(|(_, candidate)| candidate.next_retry_at_ms)
                .min()
                .unwrap_or(u64::MAX);
            let exhausted_week = member
                .weekly_exhausted_until
                .is_some_and(|until| now_ms <= until);
            rows.push((
                (
                    !active,
                    status_rank,
                    status_rank == 1 && exhausted_week,
                    if status_rank == 1 { retry } else { 0 },
                    runtime_rank,
                    saved_rank,
                ),
                key,
            ));
        }
        rows.sort_by_key(|(rank, _)| *rank);
        PoolRotationView {
            inventory_revision: 0,
            preset: self.policy.rotation_preset(),
            member_order: rows.into_iter().map(|(_, key)| key).collect(),
            member_statuses,
        }
    }
}
