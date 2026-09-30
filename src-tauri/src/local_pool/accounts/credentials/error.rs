use crate::local_pool::error::{ErrorCode, LocalPoolError};
use reqwest::header::HeaderValue;
use serde::Serialize;
use std::fmt;
use zenith_relay_core::providers::chatgpt::bearer_authorization as shared_bearer_authorization;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialErrorCode {
    InvalidIdentity,
    InvalidSecret,
    InvalidVersion,
    SecretMissing,
    SecretStoreUnavailable,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialError {
    pub code: CredentialErrorCode,
    pub message: String,
}

impl CredentialError {
    pub(super) fn new(code: CredentialErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

pub(in crate::local_pool::accounts) fn bearer_authorization(
    access_token: &str,
) -> Result<HeaderValue, CredentialError> {
    shared_bearer_authorization(access_token).map_err(|_| {
        CredentialError::new(
            CredentialErrorCode::InvalidSecret,
            "stored ChatGPT token is invalid",
        )
    })
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CredentialError {}

/// Preserves credential-store failures as user-actionable local errors.
pub(in crate::local_pool) fn credential_local_error(error: CredentialError) -> LocalPoolError {
    let code = match error.code {
        CredentialErrorCode::SecretMissing => ErrorCode::NotFound,
        CredentialErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        _ => ErrorCode::InvalidState,
    };
    LocalPoolError::new(code, error.message)
}

/// Uses the neutral operation failure surface where credential details are not actionable.
pub(in crate::local_pool) fn credential_invalid_state_error(
    error: CredentialError,
) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.message)
}
