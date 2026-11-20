//! Optional reset-credit transport and fenced redemption workflow.
pub mod snapshot;
pub mod workflow;
use crate::{ProxyConfig, providers::chatgpt::OAuthClientKind};
use reqwest::{
    StatusCode,
    header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HeaderValue, REFERER, USER_AGENT},
    redirect::Policy,
};
use serde::Serialize;
use serde_json::Value;
use std::time::Duration;

const RESET_CREDITS_URL: &str = "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits";
const RESET_CREDITS_CONSUME_URL: &str =
    "https://chatgpt.com/backend-api/wham/rate-limit-reset-credits/consume";
const MAX_RESET_CREDITS_RESPONSE_BYTES: usize = 256 * 1024;
const CHATGPT_WEB_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/154.0.0.0 Safari/537.36";

/// Connector reset support; check authorization and credits separately.
pub fn supports_reset_credits(kind: OAuthClientKind) -> bool {
    matches!(kind, OAuthClientKind::Codex | OAuthClientKind::ExcelBps)
}

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

pub struct ResetHttpResponse {
    pub status: StatusCode,
    pub response_body: Vec<u8>,
}
pub struct ResetAuthorization<'a> {
    pub client_kind: OAuthClientKind,
    pub authorization: &'a HeaderValue,
    pub provider_account_id: &'a str,
    pub proxy: Option<&'a ProxyConfig>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetErrorKind {
    Conflict,
    InvalidState,
    Unavailable,
}
#[derive(Clone, Debug)]
pub struct ResetCreditsError {
    pub kind: ResetErrorKind,
    pub message: &'static str,
    pub status: Option<u16>,
    pub retryable: Option<bool>,
}
impl ResetCreditsError {
    fn new(kind: ResetErrorKind, message: &'static str) -> Self {
        Self {
            kind,
            message,
            status: None,
            retryable: None,
        }
    }
    fn unavailable(message: &'static str) -> Self {
        Self::new(ResetErrorKind::Unavailable, message)
    }
}

pub async fn get_reset_credits(
    auth: &ResetAuthorization<'_>,
) -> Result<ResetHttpResponse, ResetCreditsError> {
    send(auth, reqwest::Method::GET, RESET_CREDITS_URL, None).await
}
pub async fn post_reset_credit(
    auth: &ResetAuthorization<'_>,
    request_id: &str,
) -> Result<ResetHttpResponse, ResetCreditsError> {
    let body = serde_json::json!({"redeem_request_id": request_id});
    send(
        auth,
        reqwest::Method::POST,
        RESET_CREDITS_CONSUME_URL,
        Some(&body),
    )
    .await
}
async fn send(
    auth: &ResetAuthorization<'_>,
    method: reqwest::Method,
    endpoint: &str,
    body: Option<&Value>,
) -> Result<ResetHttpResponse, ResetCreditsError> {
    if !supports_reset_credits(auth.client_kind) {
        return Err(ResetCreditsError::new(
            ResetErrorKind::Conflict,
            "Reset credits require a linked Codex or Excel/BPS connection",
        ));
    }
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent("Zenith Relay");
    let client = match auth.proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| ResetCreditsError::unavailable("reset credits client is unavailable"))?;
    let mut account_header = HeaderValue::from_str(auth.provider_account_id).map_err(|_| {
        ResetCreditsError::new(
            ResetErrorKind::InvalidState,
            "account provider id is invalid",
        )
    })?;
    account_header.set_sensitive(true);
    let mut request = client
        .request(method, endpoint)
        .header(AUTHORIZATION, auth.authorization.clone())
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
    use crate::scheduler::refresh::http::{HttpClass, management_http_gate};
    let (response, permit) = management_http_gate()
        .send(&client, request, HttpClass::Ordinary)
        .await
        .map_err(|_| ResetCreditsError::unavailable("reset credits request failed"))?;
    let status = response.status();
    let response_body = crate::collect_limited(response, MAX_RESET_CREDITS_RESPONSE_BYTES)
        .await
        .map_err(|_| ResetCreditsError::unavailable("reset credits response could not be read"))?;
    drop(permit);
    Ok(ResetHttpResponse {
        status,
        response_body,
    })
}
pub fn parse_reset_response(
    response: ResetHttpResponse,
) -> Result<ResetCreditsSnapshot, ResetCreditsError> {
    if !response.status.is_success() {
        return Err(reset_http_error(response.status));
    }
    if response
        .response_body
        .iter()
        .all(|byte| byte.is_ascii_whitespace())
    {
        return Ok(ResetCreditsSnapshot::default());
    }
    let payload = serde_json::from_slice(&response.response_body)
        .map_err(|_| ResetCreditsError::unavailable("reset credits response was not valid JSON"))?;
    Ok(snapshot::parse_snapshot(&payload))
}
pub fn ensure_reset_success(response: ResetHttpResponse) -> Result<(), ResetCreditsError> {
    if response.status.is_success() {
        Ok(())
    } else {
        Err(reset_http_error(response.status))
    }
}
pub fn reset_http_error(status: StatusCode) -> ResetCreditsError {
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
    ResetCreditsError {
        kind: ResetErrorKind::Unavailable,
        message,
        status: Some(status.as_u16()),
        retryable: Some(retryable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn siwc_reset_requests_stop_before_building_http() {
        let header = HeaderValue::from_static("Bearer synthetic-siwc-access");
        let auth = ResetAuthorization {
            client_kind: OAuthClientKind::Siwc,
            authorization: &header,
            provider_account_id: "synthetic-provider",
            proxy: None,
        };
        for method in [reqwest::Method::GET, reqwest::Method::POST] {
            assert!(
                matches!(send(&auth, method, "invalid-endpoint", None).await, Err(error) if error.kind == ResetErrorKind::Conflict)
            );
        }
    }
    #[test]
    fn missing_evidence_is_unknown_and_rejections_keep_status() {
        let parsed = parse_reset_response(ResetHttpResponse {
            status: StatusCode::OK,
            response_body: Vec::new(),
        })
        .unwrap();
        assert_eq!(parsed.available_count, None);
        assert_eq!(
            reset_http_error(StatusCode::TOO_MANY_REQUESTS).retryable,
            Some(true)
        );
        assert_eq!(reset_http_error(StatusCode::UNAUTHORIZED).status, Some(401));
    }
}
