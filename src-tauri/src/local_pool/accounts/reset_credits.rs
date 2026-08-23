use super::{
    credentials::CredentialStore,
    import_orchestrator::credential_local_error,
    quota_refresh::{
        ensure_local_agent_identity_task, prepare_account_request_authorization,
        recover_account_authorization, refresh_manual_account_quota, PreparedAccountAuthorization,
    },
    NativeSecretBackend,
};
use crate::local_pool::{
    commands::current_time_ms,
    error::{CommandError, ErrorCode, ErrorDiagnostics, LocalPoolError, Result as LocalResult},
    state::DesktopState,
};
use reqwest::{
    header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, REFERER, USER_AGENT},
    redirect::Policy,
    StatusCode,
};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;
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
pub struct ResetCredit {
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
pub struct ResetCreditsSnapshot {
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
pub async fn get_local_reset_credits(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<ResetCreditsSnapshot> {
    fetch_account_reset_credits(&state, &account_id)
        .await
        .map_err(Into::into)
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
    let lock = state.quota_account_lock(account_id)?;
    {
        let _guard = lock.lock().await;
        let mut prepared = prepare_account_request_authorization(state, account_id).await?;
        let available =
            fetch_reset_snapshot_with_retry(state, account_id, &mut prepared, true).await?;
        if available.available_count.unwrap_or(0) == 0 {
            return Err(LocalPoolError::new(
                ErrorCode::Conflict,
                "no reset credits are currently available for this account",
            ));
        }

        let redeem_request_id = Uuid::new_v4().to_string();
        let response = post_reset_credit(&prepared, &redeem_request_id).await?;
        if response.status == StatusCode::UNAUTHORIZED {
            prepared = retry_authorization(state, account_id, &prepared).await?;
            let retry = post_reset_credit(&prepared, &redeem_request_id).await?;
            ensure_reset_success(retry)?;
        } else {
            ensure_reset_success(response)?;
        }
    }

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

type CommandResult<T> = std::result::Result<T, CommandError>;

async fn fetch_account_reset_credits(
    state: &DesktopState,
    account_id: &str,
) -> LocalResult<ResetCreditsSnapshot> {
    let mut prepared = prepare_account_request_authorization(state, account_id).await?;
    fetch_reset_snapshot_with_retry(state, account_id, &mut prepared, true).await
}

async fn fetch_reset_snapshot_with_retry(
    state: &DesktopState,
    account_id: &str,
    prepared: &mut PreparedAccountAuthorization,
    retry_unauthorized: bool,
) -> LocalResult<ResetCreditsSnapshot> {
    let response = get_reset_credits(prepared).await?;
    if response.status == StatusCode::UNAUTHORIZED && retry_unauthorized {
        *prepared = retry_authorization(state, account_id, prepared).await?;
        let retry = get_reset_credits(prepared).await?;
        return parse_reset_response(retry);
    }
    parse_reset_response(response)
}

async fn retry_authorization(
    state: &DesktopState,
    account_id: &str,
    prepared: &PreparedAccountAuthorization,
) -> LocalResult<PreparedAccountAuthorization> {
    if prepared.tokens.is_some() {
        return PreparedAccountAuthorization::from_tokens(
            recover_account_authorization(state, account_id, current_time_ms()).await?,
        );
    }

    let stored = CredentialStore::from_backend(NativeSecretBackend)
        .require(account_id)
        .map_err(credential_local_error)?;
    let Some(task_id) = prepared.agent_task_id.as_deref() else {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "account authorization was rejected and cannot be renewed",
        ));
    };
    ensure_local_agent_identity_task(state, account_id, stored, Some(task_id)).await?;
    prepare_account_request_authorization(state, account_id).await
}

async fn get_reset_credits(
    prepared: &PreparedAccountAuthorization,
) -> LocalResult<ResetHttpResponse> {
    send_reset_request(prepared, reqwest::Method::GET, RESET_CREDITS_URL, None).await
}

async fn post_reset_credit(
    prepared: &PreparedAccountAuthorization,
    redeem_request_id: &str,
) -> LocalResult<ResetHttpResponse> {
    let body = serde_json::json!({ "redeem_request_id": redeem_request_id });
    send_reset_request(
        prepared,
        reqwest::Method::POST,
        RESET_CREDITS_CONSUME_URL,
        Some(&body),
    )
    .await
}

async fn send_reset_request(
    prepared: &PreparedAccountAuthorization,
    method: reqwest::Method,
    endpoint: &str,
    body: Option<&Value>,
) -> LocalResult<ResetHttpResponse> {
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent("Zenith Relay");
    let client = match prepared.proxy.as_ref() {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| {
        LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "reset credits client is unavailable",
        )
    })?;

    let mut account_header =
        HeaderValue::from_str(&prepared.provider_account_id).map_err(|_| {
            LocalPoolError::new(ErrorCode::InvalidState, "account provider id is invalid")
        })?;
    account_header.set_sensitive(true);
    let mut request = client
        .request(method, endpoint)
        .header(AUTHORIZATION, prepared.authorization.clone())
        .header("ChatGPT-Account-Id", account_header)
        .header(ACCEPT, "application/json")
        .header(CONTENT_TYPE, "application/json")
        .header(REFERER, "https://chatgpt.com/")
        .header(USER_AGENT, CHATGPT_WEB_USER_AGENT)
        .header("OpenAI-Beta", "codex-1")
        .header("oai-language", "en-US")
        .header("sec-fetch-site", "none")
        .header("sec-fetch-mode", "no-cors")
        .header("sec-fetch-dest", "empty")
        .header("priority", "u=4, i")
        .header("originator", "Codex Desktop");
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request.send().await.map_err(|_| {
        LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "reset credits request failed",
        )
    })?;
    let status = response.status();
    let body = super::collect_limited(response, MAX_RESET_CREDITS_RESPONSE_BYTES)
        .await
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "reset credits response could not be read",
            )
        })?;
    Ok(ResetHttpResponse { status, body })
}

fn parse_reset_response(response: ResetHttpResponse) -> LocalResult<ResetCreditsSnapshot> {
    if !response.status.is_success() {
        return Err(reset_http_error(response.status));
    }
    if response.body.iter().all(|byte| byte.is_ascii_whitespace()) {
        return Ok(ResetCreditsSnapshot::default());
    }
    let payload: Value = serde_json::from_slice(&response.body).map_err(|_| {
        LocalPoolError::new(
            ErrorCode::GatewayUnavailable,
            "reset credits response was not valid JSON",
        )
    })?;
    Ok(parse_snapshot(&payload))
}

fn ensure_reset_success(response: ResetHttpResponse) -> LocalResult<()> {
    if response.status.is_success() {
        Ok(())
    } else {
        Err(reset_http_error(response.status))
    }
}

fn reset_http_error(status: StatusCode) -> LocalPoolError {
    let (message, retryable) = match status {
        StatusCode::UNAUTHORIZED => (
            "ChatGPT authorization expired. Sign in to this account again.",
            false,
        ),
        StatusCode::FORBIDDEN => ("ChatGPT rejected reset credits for this account.", false),
        StatusCode::NOT_FOUND => ("Reset credits are not available for this account.", false),
        StatusCode::TOO_MANY_REQUESTS => (
            "ChatGPT rate-limited the reset credits request. Try again later.",
            true,
        ),
        _ if status.is_server_error() => (
            "ChatGPT reset credits service is temporarily unavailable.",
            true,
        ),
        _ => ("ChatGPT reset credits request was rejected.", false),
    };
    LocalPoolError::new(ErrorCode::GatewayUnavailable, message).with_diagnostic(ErrorDiagnostics {
        status: Some(status.as_u16()),
        retryable: Some(retryable),
        ..ErrorDiagnostics::default()
    })
}

fn parse_snapshot(payload: &Value) -> ResetCreditsSnapshot {
    let mut explicit_count = None;
    let mut raw_credits = Vec::new();
    let mut credit_list_present = false;
    collect_reset_credit_values(
        payload,
        &mut explicit_count,
        &mut raw_credits,
        &mut credit_list_present,
    );
    let credits = raw_credits
        .iter()
        .filter_map(parse_credit)
        .filter(is_codex_credit)
        .collect::<Vec<_>>();
    let available_count = explicit_count.or_else(|| {
        credit_list_present.then(|| {
            credits
                .iter()
                .filter(|credit| is_credit_available(credit))
                .count() as u32
        })
    });
    let next_expires_at = credits
        .iter()
        .filter(|credit| is_credit_available(credit))
        .filter_map(|credit| credit.expires_at)
        .min();
    ResetCreditsSnapshot {
        available_count,
        credits,
        next_expires_at,
    }
}

fn is_codex_credit(credit: &ResetCredit) -> bool {
    credit
        .reset_type
        .as_deref()
        .is_none_or(|value| value.eq_ignore_ascii_case("codex_rate_limits"))
}

fn collect_reset_credit_values(
    value: &Value,
    explicit_count: &mut Option<u32>,
    raw_credits: &mut Vec<Value>,
    credit_list_present: &mut bool,
) {
    match value {
        Value::Array(items) => {
            *credit_list_present = true;
            raw_credits.extend(items.iter().filter(|item| item.is_object()).cloned());
        }
        Value::Object(object) => {
            if explicit_count.is_none() {
                *explicit_count = ["available_count", "availableCount"]
                    .iter()
                    .find_map(|key| object.get(*key).and_then(parse_u32));
            }
            let mut nested = false;
            for key in ["credits", "rate_limit_reset_credits", "items", "data"] {
                if let Some(child) = object.get(key) {
                    nested = true;
                    collect_reset_credit_values(
                        child,
                        explicit_count,
                        raw_credits,
                        credit_list_present,
                    );
                }
            }
            if !nested && looks_like_reset_credit(object) {
                *credit_list_present = true;
                raw_credits.push(value.clone());
            }
        }
        _ => {}
    }
}

fn looks_like_reset_credit(object: &serde_json::Map<String, Value>) -> bool {
    [
        "id",
        "status",
        "state",
        "type",
        "reset_type",
        "resetType",
        "expires_at",
        "expire_at",
        "expiresAt",
        "granted_at",
        "created_at",
        "redeemed_at",
        "used_at",
        "consumed_at",
    ]
    .iter()
    .any(|key| object.contains_key(*key))
}

fn parse_credit(value: &Value) -> Option<ResetCredit> {
    let object = value.as_object()?;
    let raw_status = string_field(object, &["status", "state"]);
    Some(ResetCredit {
        status: normalized_status(raw_status.as_deref()),
        reset_type: string_field(object, &["type", "reset_type", "resetType"]),
        granted_at: timestamp_field(object, &["granted_at", "created_at", "grantedAt"]),
        expires_at: timestamp_field(object, &["expires_at", "expire_at", "expiresAt"]),
        redeemed_at: timestamp_field(
            object,
            &["redeemed_at", "used_at", "consumed_at", "redeemedAt"],
        ),
    })
}

fn string_field(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        object.get(*key).and_then(|value| match value {
            Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
            Value::Number(number) => Some(number.to_string()),
            Value::Bool(flag) => Some(flag.to_string()),
            _ => None,
        })
    })
}

fn timestamp_field(object: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<i64> {
    let value = keys.iter().find_map(|key| object.get(*key))?;
    if let Some(number) = value.as_i64() {
        return Some(normalize_timestamp(number));
    }
    let text = value.as_str()?.trim();
    if let Ok(number) = text.parse::<i64>() {
        return Some(normalize_timestamp(number));
    }
    chrono::DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|date| date.timestamp())
}

fn parse_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|value| u32::try_from(value).ok())
        .or_else(|| value.as_i64().and_then(|value| u32::try_from(value).ok()))
        .or_else(|| value.as_str()?.trim().parse::<u32>().ok())
}

fn normalize_timestamp(value: i64) -> i64 {
    if value > 1_000_000_000_000 {
        value / 1_000
    } else {
        value
    }
}

fn normalized_status(raw_status: Option<&str>) -> Option<String> {
    raw_status.map(|status| status.trim().to_ascii_lowercase())
}

fn is_credit_available(credit: &ResetCredit) -> bool {
    if credit
        .reset_type
        .as_deref()
        .is_some_and(|value| !value.eq_ignore_ascii_case("codex_rate_limits"))
    {
        return false;
    }
    let status = credit.status.as_deref().unwrap_or("available");
    if !status.is_empty() && status != "available" {
        return false;
    }
    credit
        .expires_at
        .map(|expires_at| expires_at > chrono::Utc::now().timestamp())
        .unwrap_or(true)
}

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
