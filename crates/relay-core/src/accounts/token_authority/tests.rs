use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

mod auth_refresh;
mod generations;
mod rotation;
mod slot_removal;

struct RefreshOnce {
    calls: AtomicUsize,
}

impl TokenRefreshAdapter for RefreshOnce {
    fn refresh<'a>(
        &'a self,
        _account_id: &'a str,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> BoxFuture<'a, Result<TokenRefresh, TokenRefreshFailure>> {
        Box::pin(async move {
            assert_eq!(refresh_token, "refresh-secret");
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(10)).await;
            TokenRefresh::new("new-access-secret", None, None, Some(now_ms + 60_000)).map_err(
                |_| TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid"),
            )
        })
    }
}

#[derive(Default)]
struct CapturePersistence {
    token_calls: AtomicUsize,
    auth_states: std::sync::Mutex<Vec<(String, AccountAuthState)>>,
}

impl TokenPersistenceAdapter for CapturePersistence {
    fn persist<'a>(
        &'a self,
        _account_id: &'a str,
        _tokens: &'a TokenSet,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            self.token_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> BoxFuture<'a, Result<(), TokenPersistenceFailure>> {
        Box::pin(async move {
            self.auth_states
                .lock()
                .unwrap()
                .push((account_id.to_string(), auth_state));
            Ok(())
        })
    }

    fn persist_agent_task_id<'a>(
        &'a self,
        _account_id: &'a str,
        _expected_task_id: Option<&'a str>,
        _task_id: &'a str,
    ) -> BoxFuture<'a, Result<String, TokenPersistenceFailure>> {
        Box::pin(async move { Ok(_task_id.to_string()) })
    }
}
