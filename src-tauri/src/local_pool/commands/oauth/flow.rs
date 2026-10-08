use crate::local_pool::{
    accounts::{
        oauth::{OAuthError, CODEX_OAUTH_CLIENT_ID, CODEX_OAUTH_SCOPE},
        oauth_flow::{OAuthFlowError, OAuthFlowErrorCode, OAuthFlowStart},
        records::CODEX_SOURCE_ID,
    },
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    state::DesktopState,
};

use std::collections::BTreeSet;

use url::Url;
use zenith_relay_core::url_has_userinfo;

const AUTHORIZATION_ENDPOINT: &str = "https://auth.openai.com/oauth/authorize";
const CALLBACK_PATH: &str = "/auth/callback";

pub(super) fn validate_oauth_target(
    state: &DesktopState,
    account_id: Option<&str>,
) -> LocalResult<Option<String>> {
    let Some(account_id) = account_id
        .map(str::trim)
        .filter(|account_id_text| !account_id_text.is_empty())
    else {
        return Ok(None);
    };
    let store = state.store()?;
    let account = store.account(account_id).ok_or_else(|| {
        LocalPoolError::new(ErrorCode::NotFound, "OAuth target account was not found")
    })?;
    if account.remote_location.is_some() {
        return Err(LocalPoolError::new(
            ErrorCode::Conflict,
            "OAuth target account is managed by a remote server",
        ));
    }
    if account.account.source_id != CODEX_SOURCE_ID {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "OAuth target account is not a ChatGPT account",
        ));
    }
    Ok(Some(account_id.to_string()))
}

pub(super) fn validated_authorization_url(start: &OAuthFlowStart) -> LocalResult<String> {
    let authorization = Url::parse(&start.authorization_url).map_err(|_| unsafe_oauth_url())?;
    let endpoint = Url::parse(AUTHORIZATION_ENDPOINT).map_err(|_| unsafe_oauth_url())?;
    if authorization.scheme() != endpoint.scheme()
        || authorization.host_str() != endpoint.host_str()
        || authorization.port().is_some()
        || authorization.path() != endpoint.path()
        || url_has_userinfo(&authorization)
        || authorization.fragment().is_some()
    {
        return Err(unsafe_oauth_url());
    }
    let redirect = Url::parse(&start.redirect_uri).map_err(|_| unsafe_oauth_url())?;
    if redirect.scheme() != "http"
        || redirect.host_str() != Some("localhost")
        || redirect.port().is_none()
        || redirect.path() != CALLBACK_PATH
        || url_has_userinfo(&redirect)
        || redirect.query().is_some()
        || redirect.fragment().is_some()
    {
        return Err(unsafe_oauth_url());
    }
    let mut seen = BTreeSet::new();
    for (key, value) in authorization.query_pairs() {
        let key = key.into_owned();
        if !seen.insert(key.clone()) {
            return Err(unsafe_oauth_url());
        }
        let valid = match key.as_str() {
            "response_type" => value == "code",
            "client_id" => value == CODEX_OAUTH_CLIENT_ID,
            "redirect_uri" => value == start.redirect_uri,
            "scope" => value == CODEX_OAUTH_SCOPE,
            "code_challenge" | "state" => valid_oauth_nonce(&value),
            "code_challenge_method" => value == "S256",
            "id_token_add_organizations" | "codex_cli_simplified_flow" => value == "true",
            "originator" => value == "codex_cli_rs",
            _ => false,
        };
        if !valid {
            return Err(unsafe_oauth_url());
        }
    }
    if seen.len() != 10 {
        return Err(unsafe_oauth_url());
    }
    Ok(authorization.to_string())
}

pub(super) fn valid_oauth_nonce(nonce: &str) -> bool {
    (32..=256).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

pub(super) fn unsafe_oauth_url() -> LocalPoolError {
    LocalPoolError::new(
        ErrorCode::RecoveryRequired,
        "OAuth authorization URL failed validation",
    )
}

pub(super) fn flow_error(error: OAuthFlowError) -> LocalPoolError {
    let code = match error.code {
        OAuthFlowErrorCode::CallbackAlreadyReceived => ErrorCode::Conflict,
        OAuthFlowErrorCode::Expired | OAuthFlowErrorCode::SecretMissing => ErrorCode::NotFound,
        OAuthFlowErrorCode::InvalidLoginId | OAuthFlowErrorCode::CallbackInvalid => {
            ErrorCode::InvalidState
        }
        OAuthFlowErrorCode::CallbackPortUnavailable | OAuthFlowErrorCode::ListenerUnavailable => {
            ErrorCode::GatewayUnavailable
        }
        OAuthFlowErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        OAuthFlowErrorCode::CleanupIncomplete
        | OAuthFlowErrorCode::RecoveryRequired
        | OAuthFlowErrorCode::SnapshotIo
        | OAuthFlowErrorCode::UnsupportedSnapshotVersion => ErrorCode::RecoveryRequired,
    };
    LocalPoolError::new(code, error.message)
}

pub(super) fn oauth_error(error: OAuthError) -> LocalPoolError {
    LocalPoolError::invalid_state(error)
}
