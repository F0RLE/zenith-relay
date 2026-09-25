//! Explicit ordering for transport/ownership fixtures, independent of quotas.
use super::*;
use zenith_relay_core::{
    CandidateKind, PoolMemberKind, PoolRoutingMember, PoolRoutingMode, PoolRoutingPolicy,
};

pub(super) fn set_order(gateway: &TestServer, ids: &[&str]) {
    let runtime = gateway.runtime.as_ref().unwrap();
    let snapshots = runtime.candidate_runtime_order();
    let members = ids
        .iter()
        .map(|id| {
            let candidate = snapshots
                .iter()
                .find(|candidate| candidate.candidate_id == *id)
                .unwrap();
            PoolRoutingMember {
                id: (*id).into(),
                kind: match candidate.kind {
                    CandidateKind::OAuthAccount => PoolMemberKind::Account,
                    CandidateKind::ApiSource => PoolMemberKind::Source,
                },
                weight: 1,
                max_concurrency: 0,
            }
        })
        .collect();
    assert!(
        snapshots.iter().all(|candidate| ids.iter().any(|id| {
            candidate.candidate_id == *id || candidate.candidate_id.starts_with(&format!("{id}::"))
        })),
        "fixture order must include every physical member"
    );
    runtime
        .set_pool_routing_policy(
            PoolRoutingPolicy {
                mode: PoolRoutingMode::InOrder,
                members,
                ..Default::default()
            },
            3,
        )
        .unwrap();
}
