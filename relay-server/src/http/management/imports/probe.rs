use super::super::{validation_error, ManagementError};
use crate::state::AppState;
use reqwest::header::{ACCEPT, AUTHORIZATION};
use reqwest::redirect::Policy;
use serde_json::Value;
use std::time::Duration;
use url::{Host, Url};
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::{
    push_account_id_hint, resolve_account_check_account_id, unverified_chatgpt_account_id_hints,
    AccountCheckIdentityError,
};
use zenith_relay_core::url_has_userinfo;

const DEFAULT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

const MAX_ACCOUNT_CHECK_RESPONSE_BYTES: usize = 256 * 1024;

pub(super) fn imported_account_id_hints(
    explicit_account_id: Option<&str>,
    id_token: Option<&str>,
    access_token: &str,
) -> Result<Vec<String>, ManagementError> {
    let mut hints = Vec::new();
    if let Some(account_id) = explicit_account_id {
        let account_id = clean_identifier(account_id, "account id")?;
        push_account_id_hint(&mut hints, account_id);
    }
    for token in [id_token, (!access_token.is_empty()).then_some(access_token)]
        .into_iter()
        .flatten()
    {
        for account_id in unverified_chatgpt_account_id_hints(token).into_iter() {
            push_account_id_hint(&mut hints, account_id);
        }
    }
    if hints.len() > 1 {
        return Err(ManagementError::validation(
            error_codes::ACCOUNT_IDENTITY_CLAIM_CONFLICT,
            "imported account identity claims do not agree",
        ));
    }
    Ok(hints)
}

pub(super) async fn authenticate_import_account(
    state: &AppState,
    access_token: &str,
    claimed_account_ids: &[String],
) -> Result<String, ManagementError> {
    let authorization = zenith_relay_core::providers::chatgpt::bearer_authorization(access_token)
        .map_err(|_| {
        ManagementError::validation(
            error_codes::ACCESS_TOKEN_REJECTED,
            "access token is invalid",
        )
    })?;
    let http = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(20))
        .user_agent("Zenith Relay Server")
        .build()
        .map_err(|_| {
            ManagementError::validation(
                error_codes::ACCOUNT_CHECK_UNAVAILABLE,
                "ChatGPT account lookup client could not be created",
            )
        })?;
    let (response, permit) = zenith_relay_core::scheduler::refresh::http::management_http_gate()
        .send(
            &http,
            http.get(state.config.account_check_url.clone())
                .header(AUTHORIZATION, authorization)
                .header(ACCEPT, "application/json"),
            zenith_relay_core::scheduler::refresh::http::HttpClass::Auth,
        )
        .await
        .map_err(|_| {
            ManagementError::validation(
                error_codes::ACCOUNT_CHECK_FAILED,
                "ChatGPT account lookup request failed",
            )
        })?;
    let status = response.status();
    let body = response.bytes().await.map_err(|_| {
        ManagementError::validation(
            error_codes::ACCOUNT_CHECK_FAILED,
            "ChatGPT account lookup response could not be read",
        )
    })?;
    drop(permit);
    if body.len() > MAX_ACCOUNT_CHECK_RESPONSE_BYTES {
        return Err(ManagementError::validation(
            error_codes::ACCOUNT_CHECK_RESPONSE_TOO_LARGE,
            "ChatGPT account lookup response was too large",
        ));
    }
    if !status.is_success() {
        let (code, message) = match status.as_u16() {
            401 | 403 => (
                error_codes::ACCESS_TOKEN_REJECTED,
                "ChatGPT rejected the imported access token",
            ),
            429 => (
                "account_check_rate_limited",
                "ChatGPT rate limited the account lookup request",
            ),
            _ => (
                error_codes::ACCOUNT_CHECK_FAILED,
                "ChatGPT account lookup returned an unexpected status",
            ),
        };
        return Err(ManagementError::validation(code, message));
    }
    let payload: Value = serde_json::from_slice(&body).map_err(|_| {
        ManagementError::validation(
            error_codes::ACCOUNT_CHECK_FAILED,
            "ChatGPT account lookup returned invalid JSON",
        )
    })?;
    let claimed_account_ids = claimed_account_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let account_id =
        resolve_account_check_account_id(&payload, &claimed_account_ids).map_err(|error| {
            match error {
                AccountCheckIdentityError::Missing => ManagementError::validation(
                    error_codes::PROVIDER_ACCOUNT_ID_MISSING,
                    "ChatGPT account lookup did not return an account id",
                ),
                AccountCheckIdentityError::Mismatch => ManagementError::validation(
                    error_codes::ACCOUNT_IDENTITY_MISMATCH,
                    "imported account identity does not match the authenticated account",
                ),
            }
        })?;
    clean_identifier(&account_id, "account id")
}

pub(super) fn safe_plan_type(value: String) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')))
    .then(|| value.to_ascii_lowercase())
}

pub(super) fn redact_import_label(value: String, sensitive: &[Option<&str>]) -> String {
    if contains_sensitive(&value, sensitive) {
        "Imported account".to_string()
    } else {
        value
    }
}

pub(super) fn contains_sensitive(value: &str, sensitive: &[Option<&str>]) -> bool {
    sensitive
        .iter()
        .flatten()
        .any(|secret| secret.len() >= 4 && value.contains(secret))
}

pub(super) fn validate_account_responses_url(
    value: Option<&str>,
) -> Result<String, ManagementError> {
    let value = value.unwrap_or(DEFAULT_CODEX_RESPONSES_URL).trim();
    let url =
        Url::parse(value).map_err(|_| validation_error("account responses URL is invalid"))?;
    let loopback = match url.host() {
        Some(Host::Ipv4(value)) => value.is_loopback(),
        Some(Host::Ipv6(value)) => value.is_loopback(),
        Some(Host::Domain(value)) => value.eq_ignore_ascii_case("localhost"),
        None => false,
    };
    let allowed = (url.scheme() == "https"
        && url
            .host_str()
            .is_some_and(|host| host.eq_ignore_ascii_case("chatgpt.com")))
        || (url.scheme() == "http" && loopback);
    if !allowed || url_has_userinfo(&url) || url.query().is_some() || url.fragment().is_some() {
        return Err(validation_error("account responses URL is not allowed"));
    }
    Ok(url.to_string())
}

pub(super) fn clean_identifier(value: &str, name: &str) -> Result<String, ManagementError> {
    let value = value.trim();
    if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
        Err(validation_error(format!("{name} is invalid")))
    } else {
        Ok(value.to_string())
    }
}

pub(super) fn nonempty(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}
