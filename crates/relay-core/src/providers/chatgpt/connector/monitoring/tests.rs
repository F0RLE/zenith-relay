use super::*;
use std::{collections::VecDeque, sync::Mutex};

struct State {
    current: bool,
    invalidate_on_read: bool,
    commit_fails: bool,
    recovery_fails: bool,
    reads: usize,
    recoveries: usize,
    commits: usize,
    successes: usize,
    quotas: VecDeque<QuotaRefreshOutcome>,
    models: VecDeque<Result<Vec<String>, ModelDiscoveryFailure>>,
    retry_delays: Vec<u64>,
}
struct Host(Mutex<State>, OAuthClientKind);

impl Host {
    fn new() -> Self {
        Self(
            Mutex::new(State {
                current: true,
                invalidate_on_read: false,
                commit_fails: false,
                recovery_fails: false,
                reads: 0,
                recoveries: 0,
                commits: 0,
                successes: 0,
                quotas: VecDeque::new(),
                models: VecDeque::new(),
                retry_delays: Vec::new(),
            }),
            OAuthClientKind::Codex,
        )
    }
}

impl MonitorHost for Host {
    type Authorization = OAuthClientKind;
    type Error = &'static str;

    fn validate(&self) -> Result<(), Self::Error> {
        if self.0.lock().unwrap().current {
            Ok(())
        } else {
            Err("stale")
        }
    }
    fn now_ms(&self) -> u64 {
        10_000
    }
    fn client_kind(auth: &Self::Authorization) -> OAuthClientKind {
        *auth
    }
    fn has_tokens(_: &Self::Authorization) -> bool {
        true
    }
    fn has_agent_task(_: &Self::Authorization) -> bool {
        false
    }
    async fn prepare(&self) -> Result<Self::Authorization, Self::Error> {
        Ok(self.1)
    }
    async fn recover(&self, _: &Self::Authorization) -> Result<Self::Authorization, Self::Error> {
        let mut state = self.0.lock().unwrap();
        state.recoveries += 1;
        if state.recovery_fails {
            Err("renewal")
        } else {
            Ok(self.1)
        }
    }
    async fn restore_task(
        &self,
        _: &Self::Authorization,
    ) -> Result<Self::Authorization, Self::Error> {
        panic!("no agent credential")
    }
    fn auth_state(&self) -> Result<AccountAuthState, Self::Error> {
        Ok(AccountAuthState::Active)
    }
    async fn mark_access_only_rejected(&self, _: &Self::Authorization) -> Result<(), Self::Error> {
        panic!("not access only")
    }
    fn respect_retry_after(&self, _: RefreshKind, delay_ms: u64) {
        self.0.lock().unwrap().retry_delays.push(delay_ms);
    }
}

impl QuotaMonitorHost for Host {
    type Output = (QuotaRefreshOutcome, SubscriptionObservation);

    fn initial_subscription(&self) -> Subscription {
        Subscription::default()
    }
    fn skipped(&self) -> Self::Output {
        (
            QuotaRefreshOutcome::Updated(Box::default()),
            SubscriptionObservation::Timestamped(Subscription::default()),
        )
    }
    async fn read_quota(
        &self,
        _: &Self::Authorization,
        _: &Subscription,
    ) -> Result<QuotaRefreshOutcome, Self::Error> {
        let mut state = self.0.lock().unwrap();
        state.reads += 1;
        if state.invalidate_on_read {
            state.current = false;
        }
        Ok(state
            .quotas
            .pop_front()
            .unwrap_or_else(|| QuotaRefreshOutcome::Updated(Box::default())))
    }
    async fn can_read_subscription(&self) -> Result<bool, Self::Error> {
        Ok(true)
    }
    async fn read_subscription(
        &self,
        _: &Self::Authorization,
        _: u64,
    ) -> Result<Option<CodexSubscriptionRead>, QuotaRefreshFailure> {
        Ok(Some(CodexSubscriptionRead {
            metadata: CodexSubscriptionMetadata::default(),
            fallback_failure: None,
        }))
    }
    async fn subscription_failed(
        &self,
        _: &Self::Authorization,
        _: &QuotaRefreshFailure,
        _: Duration,
    ) -> Result<(), Self::Error> {
        Ok(())
    }
    async fn subscription_succeeded(&self) -> Result<(), Self::Error> {
        let mut state = self.0.lock().unwrap();
        assert!(state.commits > 0);
        state.successes += 1;
        Ok(())
    }
    async fn commit_quota(
        &self,
        quota: QuotaRefreshOutcome,
        subscription: SubscriptionObservation,
    ) -> Result<Self::Output, Self::Error> {
        self.validate()?;
        let mut state = self.0.lock().unwrap();
        if state.commit_fails {
            return Err("publish");
        }
        state.commits += 1;
        Ok((quota, subscription))
    }
}

impl ModelMonitorHost for Host {
    async fn read_models(
        &self,
        _: &Self::Authorization,
    ) -> Result<Vec<String>, ModelDiscoveryFailure> {
        let mut state = self.0.lock().unwrap();
        state.reads += 1;
        if state.invalidate_on_read {
            state.current = false;
        }
        state
            .models
            .pop_front()
            .unwrap_or_else(|| Ok(vec!["synthetic-model".into()]))
    }
    async fn commit_models(
        &self,
        _: OAuthClientKind,
        _: Result<Vec<String>, ModelDiscoveryFailure>,
    ) -> Result<(), Self::Error> {
        self.validate()?;
        self.0.lock().unwrap().commits += 1;
        Ok(())
    }
}

fn rejected_quota() -> QuotaRefreshOutcome {
    QuotaRefreshOutcome::Failed {
        failure: QuotaRefreshFailure::new(error_codes::QUOTA_UPSTREAM, false).with_http_status(401),
        subscription: Subscription::default(),
    }
}

#[tokio::test]
async fn quota_authorization_retry_is_bounded() {
    let host = Host::new();
    host.0
        .lock()
        .unwrap()
        .quotas
        .extend([rejected_quota(), rejected_quota()]);
    let (outcome, _) = refresh_quota(&host, true).await.unwrap();
    assert!(matches!(outcome, QuotaRefreshOutcome::Failed { .. }));
    let state = host.0.lock().unwrap();
    assert_eq!(
        (
            state.reads,
            state.recoveries,
            state.commits,
            state.successes
        ),
        (2, 1, 1, 1)
    );
}

#[tokio::test]
async fn transient_renewal_is_not_reported_as_a_new_login_requirement() {
    let host = Host::new();
    {
        let mut state = host.0.lock().unwrap();
        state.recovery_fails = true;
        state.quotas.push_back(rejected_quota());
    }
    let (outcome, _) = refresh_quota(&host, true).await.unwrap();
    assert!(
        matches!(outcome, QuotaRefreshOutcome::Failed { failure, .. } if failure.code == error_codes::QUOTA_TOKEN_REFRESH && failure.retryable)
    );
}

#[tokio::test]
async fn replacement_during_quota_or_model_read_prevents_commit() {
    for models in [false, true] {
        let host = Host::new();
        host.0.lock().unwrap().invalidate_on_read = true;
        let error = if models {
            refresh_models(&host).await.err()
        } else {
            refresh_quota(&host, true).await.err()
        };
        assert_eq!(error, Some("stale"));
        assert_eq!(host.0.lock().unwrap().commits, 0);
    }
}

#[tokio::test]
async fn publication_failure_does_not_advance_subscription_freshness() {
    let host = Host::new();
    host.0.lock().unwrap().commit_fails = true;
    assert_eq!(refresh_quota(&host, true).await.err(), Some("publish"));
    assert_eq!(host.0.lock().unwrap().successes, 0);
}

#[tokio::test]
async fn siwc_has_no_unsupported_quota_poll_or_fake_commit() {
    let mut host = Host::new();
    host.1 = OAuthClientKind::Siwc;
    refresh_quota(&host, true).await.unwrap();
    let state = host.0.lock().unwrap();
    assert_eq!((state.reads, state.commits, state.successes), (0, 0, 0));
}

#[tokio::test]
async fn model_retry_preserves_provider_backoff_and_is_bounded() {
    let host = Host::new();
    let failure = ModelDiscoveryFailure {
        code: ModelDiscoveryFailureCode::Unauthorized,
        retryable: false,
        http_status: Some(401),
        retry_after_ms: Some(15_000),
    };
    host.0
        .lock()
        .unwrap()
        .models
        .extend([Err(failure.clone()), Err(failure)]);
    assert!(!refresh_models(&host).await.unwrap());
    let state = host.0.lock().unwrap();
    assert_eq!((state.reads, state.recoveries, state.commits), (2, 1, 1));
    assert_eq!(state.retry_delays, [15_000, 15_000]);
}
