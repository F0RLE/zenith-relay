use super::{quota_refresh::verify_reset_account_quota, refresh_observations::AccountRefreshScope};
use crate::local_pool::{
    error::{CommandError, LocalPoolError, Result as LocalResult},
    refresh,
    state::DesktopState,
};
use reqwest::StatusCode;
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

#[cfg(test)]
use zenith_relay_core::providers::chatgpt::connector::reset_credits::ResetCreditsSnapshot;
use zenith_relay_core::providers::chatgpt::connector::reset_credits::{
    ResetHttpResponse,
    workflow::{self, ResetHost},
};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeResetCreditResponse {
    pub refreshed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<String>,
}

#[tauri::command]
pub async fn consume_local_reset_credit(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<ConsumeResetCreditResponse> {
    consume_local_reset_credit_for_account(&state, &account_id)
        .await
        .map_err(Into::into)
}

pub(crate) async fn consume_local_reset_credit_for_account(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<ConsumeResetCreditResponse> {
    let _reset = state.reset_redemption_guard().await;
    let scope = AccountRefreshScope::capture(state, account_id).await?;
    consume_reset_credit_for_scope(state, &scope.fence).await?;

    match verify_reset_account_quota(state, &scope.fence).await {
        Ok(_) => Ok(ConsumeResetCreditResponse {
            refreshed: true,
            refresh_error: None,
        }),
        Err(error) => Ok(ConsumeResetCreditResponse {
            refreshed: false,
            refresh_error: Some(error.message),
        }),
    }
}

/// The quota job calls this action without recursively requesting itself.
pub(crate) async fn consume_reset_credit_for_scope(
    state: &DesktopState,
    fence: &crate::local_pool::store::AccountRefreshFence,
) -> LocalResult<()> {
    let fingerprint = state
        .store()?
        .account(&fence.account_id)
        .and_then(|account| {
            account
                .account
                .quota
                .secondary
                .as_ref()
                .filter(|window| window.available_basis_points == Some(0))
                .or(account.account.quota.primary.as_ref())
        })
        .filter(|window| window.available_basis_points == Some(0))
        .and_then(|window| window.exhaustion_transition_fingerprint.clone());
    consume_reset_credit_when(state, fence, fingerprint.as_deref(), |_| {
        std::future::ready(Ok(true))
    })
    .await
    .map(|_| ())
}

pub(crate) async fn consume_reset_credit_when<F, Fut>(
    state: &DesktopState,
    fence: &crate::local_pool::store::AccountRefreshFence,
    attempt_fingerprint: Option<&str>,
    should_consume: F,
) -> LocalResult<bool>
where
    F: Fn(Option<u64>) -> Fut + Sync,
    Fut: std::future::Future<Output = LocalResult<bool>> + Send,
{
    let lock = state.quota_account_lock(&fence.account_id)?;
    let _guard = lock.lock().await;
    workflow::redeem(
        &DesktopResetHost {
            state,
            fence,
            attempt_fingerprint,
            should_consume,
        },
        &Uuid::new_v4().to_string(),
    )
    .await
}

struct DesktopResetHost<'a, F> {
    state: &'a DesktopState,
    fence: &'a crate::local_pool::store::AccountRefreshFence,
    attempt_fingerprint: Option<&'a str>,
    should_consume: F,
}

impl<F, Fut> ResetHost for DesktopResetHost<'_, F>
where
    F: Fn(Option<u64>) -> Fut + Sync,
    Fut: std::future::Future<Output = LocalResult<bool>> + Send,
{
    type Authorization = super::quota_refresh::PreparedAccountAuthorization;
    type Error = LocalPoolError;

    fn validate(&self) -> LocalResult<()> {
        self.state
            .store()?
            .ensure_account_refresh_current(self.fence)
    }

    fn error(
        error: zenith_relay_core::providers::chatgpt::connector::reset_credits::ResetCreditsError,
    ) -> Self::Error {
        http::local_error(error)
    }

    async fn prepare(&self) -> LocalResult<Self::Authorization> {
        refresh::request_authorization_now(self.state, self.fence).await
    }

    async fn recover(&self, rejected: &Self::Authorization) -> LocalResult<Self::Authorization> {
        http::retry_authorization(self.state, &self.fence.account_id, rejected).await
    }

    async fn balance(&self, authorization: &Self::Authorization) -> LocalResult<ResetHttpResponse> {
        http::get_reset_credits(authorization).await
    }

    async fn record_dispatch(&self, expiry_ms: Option<u64>) -> LocalResult<bool> {
        record_reset_dispatch(self.state, self.fence, self.attempt_fingerprint, || {
            (self.should_consume)(expiry_ms)
        })
        .await
    }

    fn clear_rejected(&self) -> LocalResult<()> {
        clear_rejected_reset(
            self.state,
            self.fence,
            self.attempt_fingerprint,
            StatusCode::UNAUTHORIZED,
        )
    }

    async fn redeem(
        &self,
        authorization: &Self::Authorization,
        request_id: &str,
    ) -> LocalResult<ResetHttpResponse> {
        http::post_reset_credit(authorization, request_id).await
    }
}

async fn record_reset_dispatch<F, Fut>(
    state: &DesktopState,
    fence: &crate::local_pool::store::AccountRefreshFence,
    fingerprint: Option<&str>,
    should_consume: F,
) -> LocalResult<bool>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = LocalResult<bool>>,
{
    let _mutation = state.setup_guard().await;
    state.store()?.ensure_account_refresh_current(fence)?;
    // Recheck the member, rule and quota after waiting for settings mutations,
    // in the same critical section as the durable redemption marker.
    if !should_consume().await? {
        return Ok(false);
    }
    if let Some(fingerprint) = fingerprint {
        state.mark_weekly_reset_applied(&fence.account_id, fingerprint)?;
    }
    Ok(true)
}

fn clear_rejected_reset(
    state: &DesktopState,
    fence: &crate::local_pool::store::AccountRefreshFence,
    fingerprint: Option<&str>,
    status: StatusCode,
) -> LocalResult<()> {
    state.store()?.ensure_account_refresh_current(fence)?;
    if workflow::definitely_rejected(status)
        && let Some(fingerprint) = fingerprint
    {
        state.clear_weekly_reset_attempt(fence, fingerprint)?;
    }
    Ok(())
}

#[cfg(test)]
fn available_reset_expiry(snapshot: &ResetCreditsSnapshot) -> LocalResult<Option<u64>> {
    workflow::available_expiry(snapshot).map_err(http::local_error)
}

type CommandResult<T> = std::result::Result<T, CommandError>;

mod http;
#[cfg(test)]
use zenith_relay_core::providers::chatgpt::connector::reset_credits::snapshot;

#[cfg(test)]
use http::reset_http_error;
#[cfg(test)]
use snapshot::{is_codex_credit, parse_snapshot};

#[cfg(test)]
mod tests {
    use super::*;

    async fn record_reset_dispatch(
        state: &DesktopState,
        fence: &crate::local_pool::store::AccountRefreshFence,
        fingerprint: Option<&str>,
    ) -> LocalResult<bool> {
        super::record_reset_dispatch(state, fence, fingerprint, || std::future::ready(Ok(true)))
            .await
    }

    #[tokio::test]
    async fn policy_is_rechecked_after_waiting_for_settings_before_recording_dispatch() {
        use crate::local_pool::refresh::tests::{account, cleanup, state};
        use std::sync::atomic::{AtomicBool, Ordering};
        let state = state();
        let account = account();
        let id = account.account.id.clone();
        state.store().unwrap().upsert_account(account).unwrap();
        let (_, fence) = state.store().unwrap().account_refresh_scope(&id).unwrap();
        let allowed = AtomicBool::new(true);
        {
            let mutation = state.setup_guard().await;
            let attempt =
                super::record_reset_dispatch(&state, &fence, Some("synthetic-cycle"), || {
                    std::future::ready(Ok(allowed.load(Ordering::SeqCst)))
                });
            tokio::pin!(attempt);
            assert!(futures_util::poll!(attempt.as_mut()).is_pending());
            allowed.store(false, Ordering::SeqCst);
            drop(mutation);
            assert!(!attempt.await.unwrap());
        }
        assert!(
            !state
                .weekly_reset_was_applied(&id, "synthetic-cycle")
                .unwrap()
        );
        cleanup(state).await;
    }

    #[tokio::test]
    async fn ambiguous_redemption_outcomes_keep_the_cycle_fence() {
        use crate::local_pool::refresh::tests::{account, cleanup, state};
        let state = state();
        let account = account();
        let id = account.account.id.clone();
        state.store().unwrap().upsert_account(account).unwrap();
        let (_, fence) = state.store().unwrap().account_refresh_scope(&id).unwrap();
        for status in [
            StatusCode::OK,
            StatusCode::REQUEST_TIMEOUT,
            StatusCode::CONFLICT,
            StatusCode::TOO_EARLY,
            StatusCode::INTERNAL_SERVER_ERROR,
            StatusCode::BAD_GATEWAY,
        ] {
            record_reset_dispatch(&state, &fence, Some("synthetic-cycle"))
                .await
                .unwrap();
            clear_rejected_reset(&state, &fence, Some("synthetic-cycle"), status).unwrap();
            assert!(
                state
                    .weekly_reset_was_applied(&id, "synthetic-cycle")
                    .unwrap()
            );
        }
        for status in [
            StatusCode::UNAUTHORIZED,
            StatusCode::FORBIDDEN,
            StatusCode::NOT_FOUND,
            StatusCode::BAD_REQUEST,
            StatusCode::TOO_MANY_REQUESTS,
        ] {
            state
                .mark_weekly_reset_applied(&id, "synthetic-cycle")
                .unwrap();
            clear_rejected_reset(&state, &fence, Some("synthetic-cycle"), status).unwrap();
            assert!(
                !state
                    .weekly_reset_was_applied(&id, "synthetic-cycle")
                    .unwrap()
            );
        }
        state.mark_weekly_reset_applied(&id, "newer-cycle").unwrap();
        clear_rejected_reset(&state, &fence, Some("older-cycle"), StatusCode::FORBIDDEN).unwrap();
        assert!(state.weekly_reset_was_applied(&id, "newer-cycle").unwrap());
        cleanup(state).await;
    }

    #[test]
    fn redemption_requires_confirmed_positive_credit_availability() {
        let mut snapshot = ResetCreditsSnapshot::default();
        let unknown = available_reset_expiry(&snapshot).unwrap_err();
        assert!(unknown.message.contains("not confirmed"));
        snapshot.available_count = Some(0);
        let empty = available_reset_expiry(&snapshot).unwrap_err();
        assert!(empty.message.contains("no reset credits"));
        snapshot.available_count = Some(1);
        assert_eq!(available_reset_expiry(&snapshot).unwrap(), None);
        snapshot.next_expires_at = Some(1_800_000_000);
        assert_eq!(
            available_reset_expiry(&snapshot).unwrap(),
            Some(1_800_000_000_000)
        );
    }

    #[test]
    fn parses_camel_case_snapshot_and_expiry() {
        let payload = serde_json::json!({
            "availableCount": "2",
            "credits": [
                {"id": "a", "status": "AVAILABLE", "expiresAt": 1_800_000_000_000i64},
                {"id": "b", "status": "redeemed"}
            ]
        });
        let snapshot = parse_snapshot(&payload);
        assert_eq!(snapshot.available_count, Some(2));
        assert_eq!(snapshot.credits[0].status.as_deref(), Some("available"));
        assert_eq!(snapshot.credits[0].expires_at, Some(1_800_000_000));
        assert_eq!(snapshot.next_expires_at, Some(1_800_000_000));
    }

    #[test]
    fn parses_compatible_credit_containers_without_exposing_upstream_ids() {
        for payload in [
            serde_json::json!({"credits": [{"id": "secret", "expires_at": "2027-07-03T04:05:06Z"}]}),
            serde_json::json!({"rate_limit_reset_credits": [{"expiresAt": "2027-07-04T04:05:06Z"}]}),
            serde_json::json!({"items": [{"expires_at": "2027-07-05T04:05:06Z"}]}),
            serde_json::json!({"data": [{"expires_at": "2027-07-06T04:05:06Z"}]}),
            serde_json::json!([{"expires_at": "2027-07-07T04:05:06Z"}]),
        ] {
            let snapshot = parse_snapshot(&payload);
            assert_eq!(snapshot.available_count, Some(1));
            assert_eq!(snapshot.credits.len(), 1);
            assert!(!serde_json::to_string(&snapshot).unwrap().contains("secret"));
        }
    }

    #[test]
    fn filters_non_codex_and_non_available_credit_entries() {
        let payload = serde_json::json!({
            "credits": [
                {"type": "codex_rate_limits", "status": "available"},
                {"type": "other_feature", "status": "available"},
                {"type": "codex_rate_limits", "status": "redeemed"}
            ]
        });
        let snapshot = parse_snapshot(&payload);
        assert_eq!(snapshot.available_count, Some(1));
        assert_eq!(snapshot.credits.len(), 2);
        assert!(snapshot.credits.iter().all(is_codex_credit));
    }

    #[test]
    fn derives_count_when_upstream_omits_it() {
        let payload = serde_json::json!({
            "credits": [
                {"status": "available"},
                {"status": "expired"},
                {"status": "used"}
            ]
        });
        assert_eq!(parse_snapshot(&payload).available_count, Some(1));
    }

    #[test]
    fn status_error_does_not_include_response_body() {
        let error = reset_http_error(StatusCode::FORBIDDEN);
        assert!(!error.message.contains("token"));
        assert_eq!(
            error
                .diagnostic
                .as_deref()
                .and_then(|diagnostic| diagnostic.status),
            Some(403)
        );
    }
}
