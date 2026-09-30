use super::super::claims::ImportedIdentity;
use super::super::MAX_ACCOUNT_PROFILE_RESPONSE_BYTES;
use super::ImportedCredentialMaterial;
use crate::local_pool::accounts::import_orchestrator::{ImportItemError, ItemResult};
use crate::local_pool::accounts::{collect_limited, LimitedBodyError};
use reqwest::header::{ACCEPT, AUTHORIZATION};
use reqwest::redirect::Policy;
use std::time::Duration;
use url::Url;
use zenith_relay_core::error_codes;
use zenith_relay_core::providers::chatgpt::{
    push_account_id_hint, resolve_account_check_account_id, AccountCheckIdentityError,
};
use zenith_relay_core::ProxyConfig;

pub(in crate::local_pool::accounts) async fn resolve_import_account_identity(
    mut material: ImportedCredentialMaterial,
    endpoint: &Url,
    proxy: Option<&ProxyConfig>,
    request_timeout_seconds: u64,
) -> ItemResult<ImportedCredentialMaterial> {
    if material.access_token.is_empty() {
        return Ok(material);
    }
    // A live check confirms the account when the token still works. An expired
    // or rejected token must not block import once the document already names
    // the account: the record is stored as-is and can be signed in later.
    // A token that authenticates as a different account is still rejected.
    match lookup_import_account_id_with_hints(
        endpoint.to_owned(),
        &material.access_token,
        &material.account_id_hints,
        proxy,
        Duration::from_secs(request_timeout_seconds.max(1)),
    )
    .await
    {
        Ok(account_id) => material.provider_account_id = Some(account_id),
        Err(error)
            if error.code != error_codes::ACCOUNT_IDENTITY_MISMATCH
                && material
                    .provider_account_id
                    .as_deref()
                    .is_some_and(|account_id| !account_id.is_empty()) => {}
        Err(error) => return Err(error),
    }
    Ok(material)
}

#[cfg(test)]
pub(in crate::local_pool::accounts) async fn lookup_import_account_id(
    endpoint: Url,
    access_token: &str,
    proxy: Option<&ProxyConfig>,
    timeout: Duration,
) -> ItemResult<String> {
    lookup_import_account_id_with_hints(endpoint, access_token, &[], proxy, timeout).await
}

async fn lookup_import_account_id_with_hints(
    endpoint: Url,
    access_token: &str,
    claimed_account_ids: &[String],
    proxy: Option<&ProxyConfig>,
    timeout: Duration,
) -> ItemResult<String> {
    let authorization = crate::local_pool::accounts::credentials::bearer_authorization(
        access_token,
    )
    .map_err(|_| {
        ImportItemError::new(
            error_codes::ACCESS_TOKEN_REJECTED,
            "imported access token is invalid",
        )
    })?;
    let builder = reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(timeout)
        .user_agent("Zenith Relay");
    let http = match proxy {
        Some(proxy) => proxy.apply(builder),
        None => builder,
    }
    .build()
    .map_err(|_| {
        ImportItemError::new(
            error_codes::PROVIDER_ACCOUNT_LOOKUP_FAILED,
            "ChatGPT account lookup client could not be created",
        )
    })?;
    let (response, permit) = zenith_relay_core::scheduler::refresh::http::management_http_gate()
        .send(
            &http,
            http.get(endpoint)
                .header(AUTHORIZATION, authorization)
                .header(ACCEPT, "application/json"),
            zenith_relay_core::scheduler::refresh::http::HttpClass::Auth,
        )
        .await
        .map_err(|_| {
            ImportItemError::new(
                error_codes::PROVIDER_ACCOUNT_LOOKUP_FAILED,
                "ChatGPT account lookup request failed",
            )
        })?;
    drop(permit);
    let status = response.status();
    let body = collect_limited(response, MAX_ACCOUNT_PROFILE_RESPONSE_BYTES)
        .await
        .map_err(|error| match error {
            LimitedBodyError::Transport => ImportItemError::new(
                error_codes::PROVIDER_ACCOUNT_LOOKUP_FAILED,
                "ChatGPT account lookup response could not be read",
            ),
            LimitedBodyError::TooLarge => ImportItemError::new(
                error_codes::PROVIDER_ACCOUNT_LOOKUP_FAILED,
                "ChatGPT account lookup response was too large",
            ),
        })?;
    if !status.is_success() {
        let (code, message) = match status.as_u16() {
            401 | 403 => (
                error_codes::ACCESS_TOKEN_REJECTED,
                "ChatGPT rejected the imported access token",
            ),
            429 => (
                error_codes::ACCOUNT_PROFILE_RATE_LIMITED,
                "ChatGPT rate limited the account lookup request",
            ),
            _ => (
                error_codes::PROVIDER_ACCOUNT_LOOKUP_FAILED,
                "ChatGPT account lookup returned an unexpected status",
            ),
        };
        return Err(ImportItemError::new(code, message));
    }
    let payload: serde_json::Value = serde_json::from_slice(&body).map_err(|_| {
        ImportItemError::new(
            error_codes::PROVIDER_ACCOUNT_LOOKUP_FAILED,
            "ChatGPT account lookup returned invalid JSON",
        )
    })?;
    let claimed_account_ids = claimed_account_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    resolve_account_check_account_id(&payload, &claimed_account_ids).map_err(|error| {
        let (code, message) = match error {
            AccountCheckIdentityError::Missing => (
                error_codes::PROVIDER_ACCOUNT_ID_MISSING,
                "ChatGPT account lookup did not return an account id",
            ),
            AccountCheckIdentityError::Mismatch => (
                error_codes::ACCOUNT_IDENTITY_MISMATCH,
                "imported account identity does not match the authenticated account",
            ),
        };
        ImportItemError::new(code, message)
    })
}

pub(super) fn account_id_hints(
    item_account_id: Option<String>,
    imported_identity: &ImportedIdentity,
) -> ItemResult<Vec<String>> {
    let mut hints = Vec::with_capacity(imported_identity.account_id_hints.len() + 1);
    if let Some(account_id) = item_account_id {
        push_account_id_hint(&mut hints, account_id);
    }
    for account_id in &imported_identity.account_id_hints {
        push_account_id_hint(&mut hints, account_id.clone());
    }
    ensure_account_id_hints_are_consistent(&hints)?;
    Ok(hints)
}

pub(super) fn ensure_account_id_hints_are_consistent(hints: &[String]) -> ItemResult<()> {
    if hints.len() > 1 {
        return Err(ImportItemError::new(
            error_codes::ACCOUNT_IDENTITY_CLAIM_CONFLICT,
            "imported account identity claims do not agree",
        ));
    }
    Ok(())
}
