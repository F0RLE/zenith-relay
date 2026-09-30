use super::{
    quota_refresh::refresh_manual_account_quota, refresh_observations::AccountRefreshScope,
};
use crate::local_pool::{
    error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult},
    refresh,
    state::DesktopState,
};
use reqwest::StatusCode;
use serde::Serialize;
use tauri::State;
use uuid::Uuid;

const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const RESET_CREDITS_CONSUME_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume";
const MAX_RESET_CREDITS_RESPONSE_BYTES: usize = 256 * 1024;
const CHATGPT_WEB_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/147.0.0.0 Safari/537.36";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetCredit {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reset_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redeemed_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ResetCreditsSnapshot {
    pub available_count: Option<u32>,
    pub credits: Vec<ResetCredit>,
    pub next_expires_at: Option<i64>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumeResetCreditResponse {
    pub refreshed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_error: Option<String>,
}

struct ResetHttpResponse {
    status: StatusCode,
    body: Vec<u8>,
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
    let scope = AccountRefreshScope::capture(state, account_id).await?;
    consume_reset_credit_for_scope(state, &scope.fence).await?;

    match refresh_manual_account_quota(state, account_id).await {
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
    let account_id = &fence.account_id;
    let lock = state.quota_account_lock(account_id)?;
    {
        let _guard = lock.lock().await;
        state.store()?.ensure_account_refresh_current(fence)?;
        let mut prepared = refresh::request_authorization_now(state, fence).await?;
        let available =
            fetch_reset_snapshot_with_retry(state, account_id, &mut prepared, true).await?;
        if available.available_count.unwrap_or(0) == 0 {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "no reset credits are currently available for this account",
            ));
        }

        state.store()?.ensure_account_refresh_current(fence)?;
        let redeem_request_id = Uuid::new_v4().to_string();
        let response = post_reset_credit(&prepared, &redeem_request_id).await?;
        if response.status == StatusCode::UNAUTHORIZED {
            prepared = retry_authorization(state, account_id, &prepared).await?;
            state.store()?.ensure_account_refresh_current(fence)?;
            let retry = post_reset_credit(&prepared, &redeem_request_id).await?;
            ensure_reset_success(retry)?;
        } else {
            ensure_reset_success(response)?;
        }
    }

    Ok(())
}

type CommandResult<T> = std::result::Result<T, CommandError>;

mod http;
mod snapshot;

#[cfg(test)]
use http::reset_http_error;
use http::{
    ensure_reset_success, fetch_reset_snapshot_with_retry, post_reset_credit, retry_authorization,
};
#[cfg(test)]
use snapshot::{is_codex_credit, parse_snapshot};

#[cfg(test)]
mod tests {
    use super::*;

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
            error.diagnostic.as_deref().and_then(|value| value.status),
            Some(403)
        );
    }
}
