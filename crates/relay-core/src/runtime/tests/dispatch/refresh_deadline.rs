use super::*;
use crate::scheduler::rotation::{DispatchStartError, SharedRequestBudget};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

struct DelayedRefresh(AtomicUsize);

impl TokenRefreshAdapter for DelayedRefresh {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        _refresh_token: &'a str,
        _now_ms: u64,
    ) -> BoxFuture<'a, std::result::Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            TokenRefresh::new(
                "replacement-access",
                Some("replacement-refresh".into()),
                None,
                None,
            )
            .map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_response")
            })
        })
    }
}

#[derive(Default)]
struct SavedRefresh(Mutex<Option<TokenSet>>);

impl TokenPersistenceAdapter for SavedRefresh {
    fn persist<'a>(
        &'a self,
        _account_id: &'a str,
        tokens: &'a TokenSet,
    ) -> BoxFuture<'a, std::result::Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            *self.0.lock().unwrap() = Some(tokens.clone());
            Ok(())
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        state: AccountAuthState,
    ) -> BoxFuture<'a, std::result::Result<(), TokenPersistenceFailure>> {
        NoopPersistence.persist_auth_state(account_id, state)
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        account_id: &'a str,
        expected_task_id: Option<&'a str>,
        task_id: &'a str,
    ) -> BoxFuture<'a, std::result::Result<String, TokenPersistenceFailure>> {
        NoopPersistence.persist_agent_task_id(account_id, expected_task_id, task_id)
    }
}

#[tokio::test(start_paused = true)]
async fn refresh_at_retry_deadline_preserves_credentials_without_dispatch() {
    for http in [false, true] {
        let mut runtime = quota_runtime(QuotaSnapshot::default());
        let refresh = Arc::new(DelayedRefresh(AtomicUsize::new(0)));
        let saved = Arc::new(SavedRefresh::default());
        let account = runtime
            .account_state
            .executors
            .get_mut("account-1")
            .unwrap();
        account.refresh_adapter = refresh.clone();
        account.persistence_adapter = saved.clone();
        account.refresh_skew_ms = 0;
        let authority = account.token_authority.clone();
        let now = crate::unix_time_ms();
        authority
            .register(
                "account-1",
                TokenSet::new(
                    "expired-access",
                    Some("old-refresh".into()),
                    None,
                    Some(now - 1),
                    now - 1,
                    1,
                )
                .unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let key = runtime.authenticate_secret("local-secret").unwrap();
        let budget = SharedRequestBudget::for_incoming_request(3);
        budget.configure_retry_window(10, false);
        let lease = runtime
            .select_and_reserve_with_budget(
                &key,
                "gpt-test",
                &[WireApi::Responses],
                &HashSet::new(),
                (None, None),
                now,
                &budget,
            )
            .await
            .unwrap()
            .1;
        budget.observe_rejection();
        let prepared = runtime
            .prepare_authorization("account-1", now)
            .await
            .unwrap();
        let dispatch = if http {
            lease.begin_rotation_http_dispatch_for(&prepared, &runtime)
        } else {
            lease.begin_rotation_dispatch_for(&prepared, &runtime)
        };
        assert_eq!(dispatch, Err(DispatchStartError::BudgetExhausted));
        assert_eq!(budget.dispatches(), 0);
        assert_eq!(budget.with_budget(|budget| budget.wire_attempts()), 0);
        assert!(budget.attempted_members().is_empty());
        let retained = authority.tokens("account-1").await.unwrap();
        assert_eq!(saved.0.lock().unwrap().as_ref(), Some(&retained));
        assert_eq!(retained.refresh_token(), Some("replacement-refresh"));
        let next = runtime
            .prepare_authorization("account-1", now + 10)
            .await
            .unwrap();
        assert_eq!(next.authorization, "Bearer replacement-access");
        assert_eq!(refresh.0.load(Ordering::SeqCst), 1);
        drop(lease);
        assert_eq!(runtime.candidate_runtime_order()[0].in_flight, 0);
    }
}
