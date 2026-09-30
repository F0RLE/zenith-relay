use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn delayed_refresh_of_a_removed_slot_cannot_persist_or_authorize_the_readded_account() {
    struct PausedRefresh {
        entered: tokio::sync::Notify,
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
                self.entered.notify_one();
                self.release.notified().await;
                Ok(
                    TokenRefresh::new("stale-refreshed-access", None, None, Some(now_ms + 60_000))
                        .unwrap(),
                )
            })
        }
    }

    let authority = Arc::new(TokenAuthority::new(1).unwrap());
    let adapter = Arc::new(PausedRefresh {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    let persistence = Arc::new(CapturePersistence::default());
    let expired = TokenSet::new(
        "old-access",
        Some("old-refresh".into()),
        None,
        Some(1),
        0,
        1,
    )
    .unwrap();
    authority
        .register("account", expired, AccountAuthState::Active)
        .await
        .unwrap();
    let old = {
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
        .register(
            "account",
            TokenSet::access_only("replacement-access", None, 10).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    adapter.release.notify_one();
    assert!(matches!(
        old.await.unwrap(),
        Err(TokenAuthorityError::AccountNotFound)
    ));
    assert_eq!(persistence.token_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        authority.tokens("account").await.unwrap().access_token(),
        "replacement-access"
    );
}

#[tokio::test]
async fn removed_slot_during_token_persistence_cannot_finish_preparation() {
    struct PausedPersistence {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        capture: CapturePersistence,
    }

    impl TokenPersistenceAdapter for PausedPersistence {
        fn persist<'a>(
            &'a self,
            account_id: &'a str,
            tokens: &'a TokenSet,
        ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                self.capture.persist(account_id, tokens).await
            })
        }

        fn persist_auth_state<'a>(
            &'a self,
            account_id: &'a str,
            state: AccountAuthState,
        ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
            self.capture.persist_auth_state(account_id, state)
        }

        fn persist_agent_task_id<'a>(
            &'a self,
            account_id: &'a str,
            expected_task_id: Option<&'a str>,
            task_id: &'a str,
        ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
            self.capture
                .persist_agent_task_id(account_id, expected_task_id, task_id)
        }
    }

    let authority = Arc::new(TokenAuthority::new(1).unwrap());
    authority
        .register(
            "account",
            TokenSet::new(
                "expired",
                Some("refresh-secret".into()),
                None,
                Some(1),
                0,
                1,
            )
            .unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let persistence = Arc::new(PausedPersistence {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        capture: CapturePersistence::default(),
    });
    let old = {
        let authority = authority.clone();
        let persistence = persistence.clone();
        tokio::spawn(async move {
            authority
                .prepare_and_persist(
                    "account",
                    10,
                    0,
                    &RefreshOnce {
                        calls: AtomicUsize::new(0),
                    },
                    persistence.as_ref(),
                )
                .await
        })
    };
    persistence.entered.notified().await;
    assert!(authority.remove("account"));
    authority
        .register(
            "account",
            TokenSet::access_only("replacement", None, 10).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    persistence.release.notify_one();
    assert!(matches!(
        old.await.unwrap(),
        Err(TokenAuthorityError::AccountNotFound)
    ));
    assert_eq!(persistence.capture.token_calls.load(Ordering::SeqCst), 1);
    assert!(persistence.capture.auth_states.lock().unwrap().is_empty());
    assert_eq!(
        authority.tokens("account").await.unwrap().access_token(),
        "replacement"
    );
}

#[tokio::test]
async fn waiting_registrations_cannot_report_success_on_a_removed_slot() {
    use futures_util::poll;
    use std::task::Poll;

    let authority = TokenAuthority::new(1).unwrap();
    for case in 0..3 {
        authority
            .register(
                "account",
                TokenSet::new("original", None, None, None, 1, 1).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let old = lock(&authority.slots).get("account").cloned().unwrap();
        let held = old.lock().await;
        let mut waiting = Box::pin(async {
            let candidate = TokenSet::new("stale", None, None, None, 2, 2).unwrap();
            match case {
                0 => authority
                    .register("account", candidate, AccountAuthState::Active)
                    .await
                    .map(|_| true),
                1 => {
                    authority
                        .register_if_newer("account", candidate, AccountAuthState::Active)
                        .await
                }
                _ => {
                    authority
                        .register_if_not_stale("account", candidate, AccountAuthState::Active)
                        .await
                }
            }
        });
        assert!(matches!(poll!(waiting.as_mut()), Poll::Pending));
        assert!(authority.remove("account"));
        authority
            .register(
                "account",
                TokenSet::new("replacement", None, None, None, 1, 1).unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        drop(held);
        assert_eq!(waiting.await, Err(TokenAuthorityError::AccountNotFound));
        assert_eq!(
            authority.tokens("account").await.unwrap().access_token(),
            "replacement"
        );
        authority.remove("account");
    }
}

#[tokio::test]
async fn waiting_reads_cannot_return_credentials_from_a_removed_slot() {
    use futures_util::poll;
    use std::task::Poll;

    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "account",
            TokenSet::access_only("old-access", None, 1).unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let old = lock(&authority.slots).get("account").cloned().unwrap();
    let held = old.lock().await;
    let mut tokens = Box::pin(authority.tokens("account"));
    let mut auth_state = Box::pin(authority.auth_state("account"));
    assert!(matches!(poll!(tokens.as_mut()), Poll::Pending));
    assert!(matches!(poll!(auth_state.as_mut()), Poll::Pending));

    assert!(authority.remove("account"));
    authority
        .register(
            "account",
            TokenSet::access_only("new-access", None, 2).unwrap(),
            AccountAuthState::DegradedAccessOnly,
        )
        .await
        .unwrap();
    drop(held);

    assert!(tokens.await.is_none());
    assert!(auth_state.await.is_none());
    assert_eq!(
        authority.tokens("account").await.unwrap().access_token(),
        "new-access"
    );
    assert_eq!(
        authority.auth_state("account").await,
        Some(AccountAuthState::DegradedAccessOnly)
    );
}
