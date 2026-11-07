use super::*;
use crate::scheduler::rotation::{
    AttemptId, AttemptObservation, DispatchStartError, ExecutionObservation, HealthObservation,
    SharedRequestBudget,
};

struct AuthorizedLease {
    runtime: GatewayRuntime,
    budget: SharedRequestBudget,
    lease: CandidateLease,
    prepared: PreparedAuthorization,
    http: bool,
}

impl AuthorizedLease {
    async fn new(http: bool) -> Self {
        let runtime = quota_runtime(QuotaSnapshot::default());
        let _revision = usage_revision(&runtime);
        let key = runtime
            .authenticate(Some(&HeaderValue::from_static("Bearer local-secret")))
            .unwrap();
        let budget = SharedRequestBudget::for_incoming_request(3);
        let lease = runtime
            .select_and_reserve_with_budget(
                &key,
                "gpt-test",
                &[WireApi::Responses],
                &Default::default(),
                (None, None),
                crate::unix_time_ms(),
                &budget,
            )
            .await
            .unwrap()
            .1;
        let prepared = runtime
            .prepare_authorization("account-1", crate::unix_time_ms())
            .await
            .unwrap();
        let fixture = Self {
            runtime,
            budget,
            lease,
            prepared,
            http,
        };
        fixture.dispatch(&fixture.prepared).unwrap();
        fixture
    }

    fn dispatch(
        &self,
        prepared: &PreparedAuthorization,
    ) -> std::result::Result<AttemptId, DispatchStartError> {
        if self.http {
            self.lease
                .begin_rotation_http_dispatch_for(prepared, &self.runtime)
        } else {
            self.lease
                .begin_rotation_dispatch_for(prepared, &self.runtime)
        }
    }

    async fn replace(&self, readd: bool) -> PreparedAuthorization {
        let authority = &self.runtime.account_state.executors["account-1"].token_authority;
        let tokens = authority.tokens("account-1").await.unwrap();
        if readd {
            assert!(authority.remove("account-1"));
        }
        authority
            .register("account-1", tokens, AccountAuthState::Active)
            .await
            .unwrap();
        self.runtime
            .prepare_authorization("account-1", crate::unix_time_ms())
            .await
            .unwrap()
    }

    fn fail_after_output(&self, now: u64) {
        self.runtime.settle_rotation_failure(
            &self.lease,
            AttemptObservation {
                execution: ExecutionObservation::committed(),
                health: HealthObservation::CountableTransient {
                    provider_not_before_ms: None,
                },
            },
            Some(CooldownRequest {
                scope: "*",
                retry_at_ms: now + 60_000,
                reason: crate::scheduler::CooldownReason::Mandatory,
                provider_not_before_ms: Some(now + 60_000),
            }),
            None,
            now,
        );
    }
}

#[tokio::test]
async fn replaced_authorization_settlement_releases_capacity_without_new_login_penalties() {
    for http in [false, true] {
        for readd in [false, true] {
            let fixture = AuthorizedLease::new(http).await;
            fixture.replace(readd).await;
            let now = crate::unix_time_ms();
            fixture.fail_after_output(now);
            assert_eq!(
                fixture
                    .runtime
                    .failure_state_for("account-1", "gpt-test", now),
                (0, None, None)
            );
            assert_eq!(fixture.runtime.candidate_runtime_order()[0].in_flight, 0);
            assert_eq!(fixture.budget.dispatches(), 1);
            assert!(!fixture.budget.can_dispatch());
        }
    }
}

#[tokio::test]
async fn authorization_redispatch_uses_its_latest_successful_dispatch_revision() {
    for http in [false, true] {
        let fixture = AuthorizedLease::new(http).await;
        fixture.budget.observe_rejection();
        let refreshed = fixture.replace(false).await;
        fixture.dispatch(&refreshed).unwrap();
        assert_eq!(fixture.budget.dispatches(), 2);
        assert_eq!(
            fixture.dispatch(&fixture.prepared),
            Err(DispatchStartError::CandidateChanged)
        );
        assert_eq!(fixture.budget.dispatches(), 2);
        let now = crate::unix_time_ms();
        fixture.fail_after_output(now);
        assert_eq!(
            fixture
                .runtime
                .failure_state_for("account-1", "gpt-test", now),
            (
                1,
                Some(("*".into(), now + 60_000)),
                Some(crate::ProviderBackoff {
                    scope: "*".into(),
                    not_before_ms: now + 60_000,
                    observed_at_ms: Some(now),
                })
            )
        );
        assert_eq!(fixture.runtime.candidate_runtime_order()[0].in_flight, 0);
        assert!(!fixture.budget.can_dispatch());
    }
}

#[tokio::test]
async fn replaced_authorization_drop_preserves_unknown_execution_and_releases_capacity() {
    for http in [false, true] {
        let fixture = AuthorizedLease::new(http).await;
        fixture.replace(true).await;
        drop(fixture.lease);
        assert_eq!(fixture.runtime.candidate_runtime_order()[0].in_flight, 0);
        assert_eq!(
            fixture
                .runtime
                .failure_state_for("account-1", "gpt-test", crate::unix_time_ms()),
            (0, None, None)
        );
        assert_eq!(fixture.budget.dispatches(), 1);
        assert!(!fixture.budget.can_dispatch());
    }
}
