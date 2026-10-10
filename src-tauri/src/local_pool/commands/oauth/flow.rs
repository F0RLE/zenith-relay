use crate::local_pool::{
    accounts::{
        oauth::{validate_authorization_url, OAuthError},
        oauth_flow::{OAuthFlowError, OAuthFlowErrorCode, OAuthFlowStart},
        records::CODEX_SOURCE_ID,
    },
    error::{ErrorCode, LocalPoolError, Result as LocalResult},
    state::DesktopState,
};

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
    validate_authorization_url(
        start.client_kind,
        &start.authorization_url,
        &start.redirect_uri,
    )
    .map(|url| url.to_string())
    .map_err(|_| unsafe_oauth_url())
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
