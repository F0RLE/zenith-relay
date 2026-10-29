use super::*;

#[tokio::test]
async fn delayed_invalid_grant_cannot_block_a_readded_identical_bearer_incarnation() {
    struct PausedFailure {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    impl TokenRefreshAdapter for PausedFailure {
        fn refresh<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: u64,
        ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::InvalidGrant,
                    "invalid_grant",
                ))
            })
        }
    }
    let authority = Arc::new(TokenAuthority::new(1).unwrap());
    let tokens = TokenSet::new(
        "synthetic-same-bearer",
        Some("synthetic-refresh".into()),
        None,
        Some(1),
        0,
        1,
    )
    .unwrap();
    authority
        .register("account", tokens.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let adapter = Arc::new(PausedFailure {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let persistence = Arc::new(CapturePersistence::default());
    let task = {
        let authority = authority.clone();
        let adapter = adapter.clone();
        let persistence = persistence.clone();
        tokio::spawn(async move {
            authority
                .prepare_and_persist("account", 10, 0, adapter.as_ref(), persistence.as_ref())
                .await
        })
    };
    adapter.entered.notified().await;
    assert!(authority.remove("account"));
    authority
        .register("account", tokens.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    adapter.release.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap(),
        Err(TokenAuthorityError::AccountNotFound)
    ));
    assert_eq!(
        authority.auth_state("account").await,
        Some(AccountAuthState::Active)
    );
    let current = authority.tokens("account").await.unwrap();
    assert_eq!(current.access_token(), tokens.access_token());
    assert_eq!(current.generation(), tokens.generation());
    assert!(persistence.auth_states.lock().unwrap().is_empty());
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn old_401_cannot_invalidate_a_readded_identical_bearer_and_generation() {
    let authority = TokenAuthority::new(1).unwrap();
    let tokens = TokenSet::access_only("synthetic-same-bearer", None, 10).unwrap();
    authority
        .register("account", tokens.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let prepared = authority
        .prepare(
            "account",
            10,
            0,
            &RefreshOnce {
                calls: AtomicUsize::new(0),
            },
        )
        .await
        .unwrap();
    assert!(authority.remove("account"));
    authority
        .register("account", tokens.clone(), AccountAuthState::Active)
        .await
        .unwrap();
    let persistence = CapturePersistence::default();
    assert!(
        !authority
            .invalidate_access_revision_and_persist(
                "account",
                &prepared.dispatch_revision,
                11,
                &persistence,
            )
            .await
            .unwrap()
    );
    let current = authority.tokens("account").await.unwrap();
    assert_eq!(current.access_token(), tokens.access_token());
    assert_eq!(current.expires_at_ms(), tokens.expires_at_ms());
    assert!(persistence.auth_states.lock().unwrap().is_empty());
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn delayed_401_waits_for_refresh_and_cannot_expire_the_new_tokens() {
    struct PausedRefresh {
        started: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }
    impl TokenRefreshAdapter for PausedRefresh {
        fn refresh<'a>(
            &'a self,
            _account_id: &'a str,
            _refresh_token: &'a str,
            now_ms: u64,
        ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
            Box::pin(async move {
                self.started.notify_one();
                self.release.notified().await;
                Ok(TokenRefresh::new(
                    "synthetic-new-access",
                    Some("synthetic-new-refresh".into()),
                    None,
                    Some(now_ms + 60_000),
                )
                .unwrap())
            })
        }
    }
    let authority = Arc::new(TokenAuthority::new(1).unwrap());
    let initial = TokenSet::new(
        "synthetic-access",
        Some("synthetic-refresh".into()),
        None,
        Some(10),
        0,
        1,
    )
    .unwrap();
    authority
        .register("account", initial, AccountAuthState::Active)
        .await
        .unwrap();
    let adapter = Arc::new(PausedRefresh {
        started: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let persistence = Arc::new(CapturePersistence::default());
    let rejected = authority
        .prepare("account", 1, 0, adapter.as_ref())
        .await
        .unwrap();
    let refresh = {
        let authority = authority.clone();
        let adapter = adapter.clone();
        let persistence = persistence.clone();
        tokio::spawn(async move {
            authority
                .prepare_and_persist("account", 20, 0, adapter.as_ref(), persistence.as_ref())
                .await
        })
    };
    adapter.started.notified().await;
    let invalidation = {
        let authority = authority.clone();
        let persistence = persistence.clone();
        tokio::spawn(async move {
            authority
                .invalidate_access_revision_and_persist(
                    "account",
                    &rejected.dispatch_revision,
                    21,
                    persistence.as_ref(),
                )
                .await
        })
    };
    adapter.release.notify_one();
    refresh.await.unwrap().unwrap();
    assert!(!invalidation.await.unwrap().unwrap());
    let current = authority.tokens("account").await.unwrap();
    assert_eq!(current.refresh_token(), Some("synthetic-new-refresh"));
    assert_eq!(current.expires_at_ms(), Some(60_020));
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 1);
}
