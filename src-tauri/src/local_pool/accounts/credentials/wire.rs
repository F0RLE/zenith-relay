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

pub(super) fn validate_local_account_id(account_id: &str) -> Result<(), CredentialError> {
    if zenith_relay_core::is_ascii_token(account_id, 128) {
        Ok(())
    } else {
        Err(CredentialError::new(
            CredentialErrorCode::InvalidIdentity,
            "local Relay account id is invalid",
        ))
    }
}

pub(super) fn validate_token(token_value: &str) -> Result<(), CredentialError> {
    if token_value.is_empty()
        || token_value.len() > MAX_TOKEN_BYTES
        || token_value.bytes().any(|byte| byte.is_ascii_control())
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
    credential_value: Option<&str>,
    max_bytes: usize,
) -> Result<(), CredentialError> {
    if credential_value.is_some_and(|credential_text| {
        credential_text.is_empty()
            || credential_text.len() > max_bytes
            || credential_text.bytes().any(|byte| byte.is_ascii_control())
    }) {
        Err(CredentialError::new(
            CredentialErrorCode::InvalidSecret,
            "stored ChatGPT credential metadata is invalid",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn mask_email(email_value: &str) -> String {
    let Some((local, domain)) = email_value.trim().split_once('@') else {
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
