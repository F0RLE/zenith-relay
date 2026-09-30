use super::*;
use crate::scheduler::rotation::{AttemptObservation, ExecutionObservation, HealthObservation};
use crate::scheduler::CooldownReason;
use crate::{
    CandidateHealth, CandidateQuota, PoolMemberKind, PoolRoutingMember, PoolRoutingMode,
    PoolRoutingPolicy,
};

mod fences;
mod observation;
mod principal_scope;

fn runtime() -> Arc<GatewayRuntime> {
    Arc::new(
        GatewayRuntime::from_pool(
            ["source-a", "source-b"]
                .into_iter()
                .map(|id| {
                    RuntimeSource::unrestricted(ProviderSource {
                        id: id.into(),
                        name: id.into(),
                        base_url: "https://example.test/v1".into(),
                        api_key: "synthetic-source-key".into(),
                        wire_api: WireApi::Responses,
                        models: vec!["model-a".into(), "model-b".into()],
                    })
                })
                .collect(),
            vec![RuntimeLocalKey::unrestricted(LocalGatewayKey {
                id: "key".into(),
                secret: "synthetic-local-key".into(),
            })],
            GatewayRuntimeOptions {
                pool_routing: Some(PoolRoutingPolicy {
                    mode: PoolRoutingMode::InOrder,
                    members: ["source-a", "source-b"]
                        .into_iter()
                        .map(|id| PoolRoutingMember {
                            kind: PoolMemberKind::Source,
                            id: id.into(),
                            weight: 1,
                            max_concurrency: 4,
                        })
                        .collect(),
                    ..PoolRoutingPolicy::default()
                }),
                ..GatewayRuntimeOptions::default()
            },
            Arc::new(|_| {}),
        )
        .unwrap(),
    )
}

fn key(runtime: &GatewayRuntime) -> AuthenticatedKey {
    runtime
        .authenticate(Some(&HeaderValue::from_static(
            "Bearer synthetic-local-key",
        )))
        .unwrap()
}

async fn reserve(
    runtime: &GatewayRuntime,
    budget: &SharedRequestBudget,
    protocol: WireApi,
) -> CandidateLease {
    runtime
        .select_and_reserve_with_budget(
            &key(runtime),
            "model-a",
            &[protocol],
            &HashSet::new(),
            (None, None),
            crate::unix_time_ms(),
            budget,
        )
        .await
        .unwrap()
        .1
}

fn rejected() -> AttemptObservation {
    AttemptObservation {
        execution: ExecutionObservation::not_sent(),
        health: HealthObservation::ClientError,
    }
}
