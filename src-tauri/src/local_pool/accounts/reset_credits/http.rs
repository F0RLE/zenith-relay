use super::super::{
    credentials::CredentialStore,
    import_orchestrator::credential_local_error,
    quota_refresh::{
        ensure_local_agent_identity_task, prepare_account_request_authorization,
        recover_account_authorization, PreparedAccountAuthorization,
    },
    NativeSecretBackend,
};
use super::snapshot::parse_snapshot;
use super::{
    ResetCreditsSnapshot, ResetHttpResponse, CHATGPT_WEB_USER_AGENT,
    MAX_RESET_CREDITS_RESPONSE_BYTES, RESET_CREDITS_CONSUME_URL, RESET_CREDITS_URL,
};
use crate::local_pool::{
    commands::current_time_ms,
    error::{ErrorCode, ErrorDiagnostics, LocalPoolError, Result as LocalResult},
    state::DesktopState,
};
use reqwest::{
    header::{HeaderValue, ACCEPT, AUTHORIZATION, CONTENT_TYPE, REFERER, USER_AGENT},
    redirect::Policy,
    StatusCode,
};
use serde_json::Value;
use std::time::Duration;
use zenith_relay_core::scheduler::refresh::http::{management_http_gate, HttpClass};

pub(super) async fn fetch_reset_snapshot_with_retry(
    state: &DesktopState,
    account_id: &str,
    prepared: &mut PreparedAccountAuthorization,
    retry_unauthorized: bool,
) -> LocalResult<ResetCreditsSnapshot> {
    let reset_response = get_reset_credits(prepared).await?;
    if reset_response.status == StatusCode::UNAUTHORIZED && retry_unauthorized {
        *prepared = retry_authorization(state, account_id, prepared).await?;
        let retry_response = get_reset_credits(prepared).await?;
        return parse_reset_response(retry_response);
    }
    parse_reset_response(reset_response)
}

pub(super) async fn retry_authorization(
    state: &DesktopState,
    account_id: &str,
    prepared: &PreparedAccountAuthorization,
) -> LocalResult<PreparedAccountAuthorization> {
    if prepared.tokens.is_some() {
        return PreparedAccountAuthorization::from_tokens(
            recover_account_authorization(
                state,
                account_id,
                prepared.tokens.as_ref().map(|tokens| tokens.generation()),
                current_time_ms(),
            )
            .await?,
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

pub(super) async fn post_reset_credit(
    prepared: &PreparedAccountAuthorization,
    redeem_request_id: &str,
) -> LocalResult<ResetHttpResponse> {
    let redeem_request_body = serde_json::json!({ "redeem_request_id": redeem_request_id });
    send_reset_request(
        prepared,
        reqwest::Method::POST,
        RESET_CREDITS_CONSUME_URL,
        Some(&redeem_request_body),
    )
    .await
}

async fn send_reset_request(
    prepared: &PreparedAccountAuthorization,
    method: reqwest::Method,
    endpoint: &str,
    request_body: Option<&Value>,
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
    let mut reset_request = client
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
    if let Some(request_body) = request_body {
        reset_request = reset_request.json(request_body);
    }
    let (reset_response, permit) = management_http_gate()
        .send(&client, reset_request, HttpClass::Ordinary)
        .await
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "reset credits request failed",
            )
        })?;
    let status = reset_response.status();
    let response_body =
        super::super::collect_limited(reset_response, MAX_RESET_CREDITS_RESPONSE_BYTES)
            .await
            .map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::GatewayUnavailable,
                    "reset credits response could not be read",
                )
            })?;
    drop(permit);
    Ok(ResetHttpResponse {
        status,
        response_body,
    })
}

fn parse_reset_response(reset_response: ResetHttpResponse) -> LocalResult<ResetCreditsSnapshot> {
    if !reset_response.status.is_success() {
        return Err(reset_http_error(reset_response.status));
    }
    if reset_response
        .response_body
        .iter()
        .all(|byte| byte.is_ascii_whitespace())
    {
        return Ok(ResetCreditsSnapshot::default());
    }
    let reset_response_payload: Value = serde_json::from_slice(&reset_response.response_body)
        .map_err(|_| {
            LocalPoolError::new(
                ErrorCode::GatewayUnavailable,
                "reset credits response was not valid JSON",
            )
        })?;
    Ok(parse_snapshot(&reset_response_payload))
}

pub(super) fn ensure_reset_success(response: ResetHttpResponse) -> LocalResult<()> {
    if response.status.is_success() {
        Ok(())
    } else {
        Err(reset_http_error(response.status))
    }
}

pub(super) fn reset_http_error(status: StatusCode) -> LocalPoolError {
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
