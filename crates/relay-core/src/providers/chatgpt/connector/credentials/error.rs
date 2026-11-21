use crate::providers::chatgpt::bearer_authorization as shared_bearer_authorization;
use reqwest::header::HeaderValue;
use serde::Serialize;
use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialErrorCode {
    InvalidIdentity,
    InvalidSecret,
    InvalidVersion,
    SecretMissing,
    SecretStoreUnavailable,
    RotationOutcomeUnknown,
    RotationCheckpointInvalid,
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

pub fn bearer_authorization(access_token: &str) -> Result<HeaderValue, CredentialError> {
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
