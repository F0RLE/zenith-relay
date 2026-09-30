use super::error::{CredentialError, CredentialErrorCode};
use serde::{Deserialize, Serialize};

pub(super) const CREDENTIAL_VERSION: u32 = 1;
pub(super) const MAX_SECRET_JSON_BYTES: usize = 256 * 1024;
pub(super) const MAX_TOKEN_BYTES: usize = 64 * 1024;
pub(super) const MAX_ID_BYTES: usize = 256;
pub(super) const MAX_EMAIL_BYTES: usize = 320;
pub(super) const MAX_PLAN_BYTES: usize = 64;

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CredentialWire {
    pub(super) version: u32,
    pub(super) local_account_id: String,
    pub(super) access_token: String,
    pub(super) refresh_token: Option<String>,
    pub(super) id_token: Option<String>,
    pub(super) expires_at_ms: Option<u64>,
    pub(super) issued_at_ms: u64,
    pub(super) generation: u64,
    pub(super) email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) totp_secret: Option<String>,
    pub(super) provider_account_id: Option<String>,
    pub(super) provider_user_id: Option<String>,
    pub(super) organization_id: Option<String>,
    pub(super) plan_type: Option<String>,
    pub(super) account_is_fedramp: bool,
    #[serde(default)]
    pub(super) proxy_url: Option<String>,
    #[serde(default)]
    pub(super) bypass_common_proxy: bool,
    #[serde(default)]
    pub(super) agent_identity: Option<AgentIdentityWire>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AgentIdentityWire {
    pub(super) private_key: String,
    pub(super) runtime_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) task_id: Option<String>,
}

pub(super) fn validate_local_account_id(value: &str) -> Result<(), CredentialError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(CredentialError::new(
            CredentialErrorCode::InvalidIdentity,
            "local Relay account id is invalid",
        ))
    }
}

pub(super) fn validate_token(value: &str) -> Result<(), CredentialError> {
    if value.is_empty()
        || value.len() > MAX_TOKEN_BYTES
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(CredentialError::new(
            CredentialErrorCode::InvalidSecret,
            "stored ChatGPT token is invalid",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn validate_optional(
    value: Option<&str>,
    max_bytes: usize,
) -> Result<(), CredentialError> {
    if value.is_some_and(|value| {
        value.is_empty()
            || value.len() > max_bytes
            || value.bytes().any(|byte| byte.is_ascii_control())
    }) {
        Err(CredentialError::new(
            CredentialErrorCode::InvalidSecret,
            "stored ChatGPT credential metadata is invalid",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn mask_email(value: &str) -> String {
    let Some((local, domain)) = value.trim().split_once('@') else {
        return "****".to_string();
    };
    let local = local.chars().next().unwrap_or('*');
    let (domain, suffix) = domain.rsplit_once('.').unwrap_or((domain, ""));
    let domain = domain.chars().next().unwrap_or('*');
    if suffix.is_empty() {
        format!("{local}***@{domain}***")
    } else {
        format!("{local}***@{domain}***.{suffix}")
    }
}
