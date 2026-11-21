use super::*;

struct ScheduledRefresh {
    calls: AtomicUsize,
    retry_at_ms: u64,
}

impl TokenRefreshAdapter for ScheduledRefresh {
    fn refresh<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            if now_ms < self.retry_at_ms {
                Err(TokenRefreshFailure::deferred(
                    "siwc_refresh_too_early",
                    self.retry_at_ms,
                ))
            } else {
                Ok(TokenRefresh::new(
                    "renewed-access",
                    Some("renewed-refresh".into()),
                    None,
                    Some(now_ms + 60_000),
                )
                .unwrap())
            }
        })
    }
}

fn tokens(expiry: u64) -> TokenSet {
    TokenSet::new(
        "synthetic-access",
        Some("synthetic-refresh".into()),
        None,
        Some(expiry),
        0,
        0,
    )
    .unwrap()
}

#[tokio::test]
async fn provider_renewal_window_preserves_access_and_schedules_one_attempt() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register("account", tokens(10_000), AccountAuthState::Active)
        .await
        .unwrap();
    let adapter = ScheduledRefresh {
        calls: AtomicUsize::new(0),
        retry_at_ms: 12_000,
    };
    let persistence = CapturePersistence::default();
    for now_ms in [100, 200, 5_000, 9_999] {
        let prepared = authority
            .prepare_and_persist("account", now_ms, 20_000, &adapter, &persistence)
            .await
            .unwrap();
        assert_eq!(prepared.status, PrepareStatus::Ready);
        assert_eq!(prepared.tokens.access_token(), "synthetic-access");
    }
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    for now_ms in [10_000, 11_999] {
        assert_eq!(
            authority
                .prepare_and_persist("account", now_ms, 20_000, &adapter, &persistence)
                .await
                .unwrap_err(),
            TokenAuthorityError::RefreshDeferred {
                retry_at_ms: 12_000
            }
        );
    }
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert!(persistence.auth_states.lock().unwrap().is_empty());
    let prepared = authority
        .prepare_and_persist("account", 12_000, 20_000, &adapter, &persistence)
        .await
        .unwrap();
    assert_eq!(prepared.status, PrepareStatus::Refreshed);
    assert_eq!(prepared.tokens.access_token(), "renewed-access");
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
}

struct FailedRefresh {
    calls: AtomicUsize,
    kind: TokenRefreshFailureKind,
    delay: Duration,
}

impl FailedRefresh {
    fn new(kind: TokenRefreshFailureKind) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            kind,
            delay: Duration::ZERO,
        }
    }
}

impl TokenRefreshAdapter for FailedRefresh {
    fn refresh<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            Err(TokenRefreshFailure::new(
                self.kind,
                "synthetic_refresh_failure",
            ))
        })
    }
}

#[tokio::test]
async fn unknown_renewal_preserves_only_unexpired_access_and_persists_access_only_state() {
    let authority = TokenAuthority::new(1).unwrap();
    let before = tokens(10_000);
    authority
        .register("account", before.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let adapter = FailedRefresh::new(TokenRefreshFailureKind::RotationOutcomeUnknown);
    let persistence = CapturePersistence::default();
    for now_ms in [100, 200, 9_999] {
        let prepared = authority
            .prepare_and_persist("account", now_ms, 20_000, &adapter, &persistence)
            .await
            .unwrap();
        assert_eq!(prepared.tokens, before);
    }
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *persistence.auth_states.lock().unwrap(),
        vec![("account".into(), AccountAuthState::DegradedAccessOnly)]
    );
    let restarted = TokenAuthority::new(1).unwrap();
    restarted
        .register("account", before, AccountAuthState::DegradedAccessOnly)
        .await
        .unwrap();
    assert!(
        restarted
            .prepare("account", 9_999, 20_000, &adapter)
            .await
            .is_ok()
    );
    for now_ms in [10_000, 10_001] {
        assert_eq!(
            restarted
                .prepare_and_persist("account", now_ms, 0, &adapter, &persistence)
                .await
                .unwrap_err(),
            TokenAuthorityError::RequiresReauth(ReauthReason::TokenRotationRecoveryRequired)
        );
    }
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn new_login_discards_a_previous_renewal_deferral() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register("account", tokens(10_000), AccountAuthState::Active)
        .await
        .unwrap();
    let adapter = ScheduledRefresh {
        calls: AtomicUsize::new(0),
        retry_at_ms: 12_000,
    };
    assert!(
        authority
            .prepare("account", 100, 20_000, &adapter)
            .await
            .is_ok()
    );
    authority
        .register("account", tokens(1), AccountAuthState::Active)
        .await
        .unwrap();
    assert!(matches!(
        authority.prepare("account", 200, 0, &adapter).await,
        Err(TokenAuthorityError::RefreshDeferred { .. })
    ));
    assert_eq!(adapter.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn terminal_grant_and_identity_errors_block_even_unexpired_access() {
    for (kind, reason) in [
        (
            TokenRefreshFailureKind::InvalidGrant,
            ReauthReason::InvalidGrant,
        ),
        (
            TokenRefreshFailureKind::InvalidatedRefreshToken,
            ReauthReason::InvalidatedRefreshToken,
        ),
        (
            TokenRefreshFailureKind::RotationRecoveryRequired,
            ReauthReason::TokenRotationRecoveryRequired,
        ),
    ] {
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register("account", tokens(10_000), AccountAuthState::Active)
            .await
            .unwrap();
        let adapter = FailedRefresh::new(kind);
        let expected = TokenAuthorityError::RequiresReauth(reason);
        assert_eq!(
            authority
                .prepare("account", 100, 20_000, &adapter)
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(
            authority
                .prepare("account", 101, 0, &adapter)
                .await
                .unwrap_err(),
            expected
        );
        assert_eq!(adapter.calls.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn uncertain_renewal_checks_expiry_after_the_exchange_wait() {
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register("account", tokens(105), AccountAuthState::Active)
        .await
        .unwrap();
    let mut adapter = FailedRefresh::new(TokenRefreshFailureKind::RotationOutcomeUnknown);
    adapter.delay = Duration::from_millis(20);
    assert_eq!(
        authority
            .prepare("account", 100, 20_000, &adapter)
            .await
            .unwrap_err(),
        TokenAuthorityError::RequiresReauth(ReauthReason::TokenRotationRecoveryRequired)
    );
}
