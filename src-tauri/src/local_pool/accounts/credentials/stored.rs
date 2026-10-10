use super::error::{bearer_authorization, CredentialError, CredentialErrorCode};
use super::wire::{
    mask_email, validate_local_account_id, validate_optional, validate_token, AgentIdentityWire,
    CredentialWire, CREDENTIAL_VERSION, MAX_EMAIL_BYTES, MAX_ID_BYTES, MAX_PLAN_BYTES,
    MAX_TOKEN_BYTES,
};
use crate::local_pool::accounts::oauth::OAuthClientKind;
use reqwest::header::HeaderValue;
use serde::Serialize;
use std::fmt;
use zenith_relay_core::accounts::access_token_is_usable;
use zenith_relay_core::omit_blank;
use zenith_relay_core::providers::chatgpt::{AgentIdentityCredential, BasisPointsCapturedHeaders};

mod login;
mod refresh;
mod secret;
mod tokens;

pub use refresh::CredentialRefresh;

#[derive(Clone)]
pub struct StoredCodexCredentials {
    version: u32,
    oauth_client_kind: OAuthClientKind,
    local_account_id: String,
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
    issued_at_ms: u64,
    generation: u64,
    email: Option<String>,
    phone: Option<String>,
    password: Option<String>,
    totp_secret: Option<String>,
    provider_account_id: Option<String>,
    provider_user_id: Option<String>,
    organization_id: Option<String>,
    plan_type: Option<String>,
    account_is_fedramp: bool,
    proxy_url: Option<String>,
    bypass_common_proxy: bool,
    agent_identity: Option<AgentIdentityCredential>,
    basis_points_headers: Option<BasisPointsCapturedHeaders>,
}

impl StoredCodexCredentials {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        local_account_id: &str,
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_at_ms: Option<u64>,
        issued_at_ms: u64,
        generation: u64,
        email: Option<String>,
        provider_account_id: Option<String>,
        provider_user_id: Option<String>,
        organization_id: Option<String>,
        plan_type: Option<String>,
        account_is_fedramp: bool,
    ) -> Result<Self, CredentialError> {
        validate_local_account_id(local_account_id)?;
        validate_token(&access_token)?;
        validate_optional(refresh_token.as_deref(), MAX_TOKEN_BYTES)?;
        validate_optional(id_token.as_deref(), MAX_TOKEN_BYTES)?;
        validate_optional(email.as_deref(), MAX_EMAIL_BYTES)?;
        validate_optional(provider_account_id.as_deref(), MAX_ID_BYTES)?;
        validate_optional(provider_user_id.as_deref(), MAX_ID_BYTES)?;
        validate_optional(organization_id.as_deref(), MAX_ID_BYTES)?;
        validate_optional(plan_type.as_deref(), MAX_PLAN_BYTES)?;
        Ok(Self {
            version: CREDENTIAL_VERSION,
            oauth_client_kind: OAuthClientKind::Codex,
            local_account_id: local_account_id.to_string(),
            access_token,
            refresh_token: omit_blank(refresh_token),
            id_token: omit_blank(id_token),
            expires_at_ms,
            issued_at_ms,
            generation,
            email: omit_blank(email),
            phone: None,
            password: None,
            totp_secret: None,
            provider_account_id: omit_blank(provider_account_id),
            provider_user_id: omit_blank(provider_user_id),
            organization_id: omit_blank(organization_id),
            plan_type: omit_blank(plan_type),
            account_is_fedramp,
            proxy_url: None,
            bypass_common_proxy: false,
            agent_identity: None,
            basis_points_headers: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_agent_identity(
        local_account_id: &str,
        agent_identity: AgentIdentityCredential,
        issued_at_ms: u64,
        generation: u64,
        email: Option<String>,
        provider_account_id: Option<String>,
        provider_user_id: Option<String>,
        organization_id: Option<String>,
        plan_type: Option<String>,
        account_is_fedramp: bool,
    ) -> Result<Self, CredentialError> {
        validate_local_account_id(local_account_id)?;
        validate_optional(email.as_deref(), MAX_EMAIL_BYTES)?;
        validate_optional(provider_account_id.as_deref(), MAX_ID_BYTES)?;
        validate_optional(provider_user_id.as_deref(), MAX_ID_BYTES)?;
        validate_optional(organization_id.as_deref(), MAX_ID_BYTES)?;
        validate_optional(plan_type.as_deref(), MAX_PLAN_BYTES)?;
        Ok(Self {
            version: CREDENTIAL_VERSION,
            oauth_client_kind: OAuthClientKind::Codex,
            local_account_id: local_account_id.to_string(),
            access_token: String::new(),
            refresh_token: None,
            id_token: None,
            expires_at_ms: None,
            issued_at_ms,
            generation,
            email: omit_blank(email),
            phone: None,
            password: None,
            totp_secret: None,
            provider_account_id: omit_blank(provider_account_id),
            provider_user_id: omit_blank(provider_user_id),
            organization_id: omit_blank(organization_id),
            plan_type: omit_blank(plan_type),
            account_is_fedramp,
            proxy_url: None,
            bypass_common_proxy: false,
            agent_identity: Some(agent_identity),
            basis_points_headers: None,
        })
    }

    pub fn local_account_id(&self) -> &str {
        &self.local_account_id
    }

    pub fn oauth_client_kind(&self) -> OAuthClientKind {
        self.oauth_client_kind
    }

    pub fn with_oauth_client_kind(mut self, kind: OAuthClientKind) -> Self {
        self.oauth_client_kind = kind;
        self
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn agent_identity(&self) -> Option<&AgentIdentityCredential> {
        self.agent_identity.as_ref()
    }

    pub fn with_agent_identity(mut self, agent_identity: AgentIdentityCredential) -> Self {
        self.agent_identity = Some(agent_identity);
        self
    }

    pub fn with_agent_task_id(&self, task_id: String) -> Result<Self, CredentialError> {
        let agent = self.agent_identity.as_ref().ok_or_else(|| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored credential is not an Agent Identity",
            )
        })?;
        let mut updated = self.clone();
        updated.agent_identity = Some(agent.with_task_id(task_id).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "Agent Identity task id is invalid",
            )
        })?);
        Ok(updated)
    }

    pub fn is_agent_identity(&self) -> bool {
        self.agent_identity.is_some()
    }

    pub fn basis_points_headers(&self) -> Option<&BasisPointsCapturedHeaders> {
        self.basis_points_headers.as_ref()
    }

    pub fn with_basis_points_headers(
        mut self,
        headers: Option<BasisPointsCapturedHeaders>,
    ) -> Result<Self, CredentialError> {
        if let Some(headers) = headers.as_ref() {
            headers.validate().map_err(|_| {
                CredentialError::new(
                    CredentialErrorCode::InvalidSecret,
                    "stored Basis Points headers are invalid",
                )
            })?;
        }
        self.basis_points_headers = headers;
        Ok(self)
    }

    pub fn has_oauth(&self) -> bool {
        !self.access_token.is_empty()
    }

    pub fn authorization(&self, now_ms: u64) -> Result<HeaderValue, CredentialError> {
        if let Some(agent) = self.agent_identity.as_ref() {
            return agent.authorization(now_ms).map_err(|_| {
                CredentialError::new(
                    CredentialErrorCode::InvalidSecret,
                    "stored Agent Identity credential is invalid",
                )
            });
        }
        bearer_authorization(&self.access_token)
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub fn id_token(&self) -> Option<&str> {
        self.id_token.as_deref()
    }

    pub fn expires_at_ms(&self) -> Option<u64> {
        self.expires_at_ms
    }

    pub fn issued_at_ms(&self) -> u64 {
        self.issued_at_ms
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn provider_account_id(&self) -> Option<&str> {
        self.provider_account_id.as_deref()
    }

    pub fn provider_user_id(&self) -> Option<&str> {
        self.provider_user_id.as_deref()
    }

    pub fn organization_id(&self) -> Option<&str> {
        self.organization_id.as_deref()
    }

    pub fn email(&self) -> Option<&str> {
        self.email.as_deref()
    }

    pub fn phone(&self) -> Option<&str> {
        self.phone.as_deref()
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub fn totp_secret(&self) -> Option<&str> {
        self.totp_secret.as_deref()
    }

    pub fn plan_type(&self) -> Option<&str> {
        self.plan_type.as_deref()
    }

    pub fn expire_access_at(&mut self, now_ms: u64) {
        self.expires_at_ms = Some(now_ms);
    }

    pub fn is_access_usable(&self, now_ms: u64, refresh_skew_ms: u64) -> bool {
        access_token_is_usable(self.expires_at_ms, now_ms, refresh_skew_ms)
    }
}

impl fmt::Debug for StoredCodexCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredCodexCredentials")
            .field("version", &self.version)
            .field("oauth_client_kind", &self.oauth_client_kind)
            .field("local_account_id", &self.local_account_id)
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at_ms", &self.expires_at_ms)
            .field("issued_at_ms", &self.issued_at_ms)
            .field("generation", &self.generation)
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("phone", &self.phone.as_ref().map(|_| "[redacted]"))
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field(
                "totp_secret",
                &self.totp_secret.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "provider_account_id",
                &self.provider_account_id.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "provider_user_id",
                &self.provider_user_id.as_ref().map(|_| "[redacted]"),
            )
            .field(
                "organization_id",
                &self.organization_id.as_ref().map(|_| "[redacted]"),
            )
            .field("plan_type", &self.plan_type)
            .field("account_is_fedramp", &self.account_is_fedramp)
            .field("proxy_url", &self.proxy_url.as_ref().map(|_| "[redacted]"))
            .field("bypass_common_proxy", &self.bypass_common_proxy)
            .field("agent_identity", &self.agent_identity)
            .field(
                "basis_points_headers",
                &self.basis_points_headers.as_ref().map(|_| "[redacted]"),
            )
            .finish()
    }
}

impl From<&StoredCodexCredentials> for CredentialWire {
    fn from(credentials: &StoredCodexCredentials) -> Self {
        Self {
            version: credentials.version,
            oauth_client_kind: credentials.oauth_client_kind,
            local_account_id: credentials.local_account_id.clone(),
            access_token: credentials.access_token.clone(),
            refresh_token: credentials.refresh_token.clone(),
            id_token: credentials.id_token.clone(),
            expires_at_ms: credentials.expires_at_ms,
            issued_at_ms: credentials.issued_at_ms,
            generation: credentials.generation,
            email: credentials.email.clone(),
            phone: credentials.phone.clone(),
            password: credentials.password.clone(),
            totp_secret: credentials.totp_secret.clone(),
            provider_account_id: credentials.provider_account_id.clone(),
            provider_user_id: credentials.provider_user_id.clone(),
            organization_id: credentials.organization_id.clone(),
            plan_type: credentials.plan_type.clone(),
            account_is_fedramp: credentials.account_is_fedramp,
            proxy_url: credentials.proxy_url.clone(),
            bypass_common_proxy: credentials.bypass_common_proxy,
            agent_identity: credentials
                .agent_identity
                .as_ref()
                .map(|agent| AgentIdentityWire {
                    private_key: agent.private_key().to_string(),
                    runtime_id: agent.runtime_id().to_string(),
                    task_id: agent.task_id().map(str::to_string),
                }),
            basis_points_headers: credentials.basis_points_headers.clone(),
        }
    }
}

impl StoredCodexCredentials {
    pub fn proxy_url(&self) -> Option<&str> {
        self.proxy_url.as_deref()
    }

    pub fn bypass_common_proxy(&self) -> bool {
        self.bypass_common_proxy
    }

    pub fn with_proxy_route(
        mut self,
        proxy_url: Option<String>,
        bypass_common_proxy: bool,
    ) -> Result<Self, CredentialError> {
        if proxy_url.is_some() && bypass_common_proxy {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "account proxy route is ambiguous",
            ));
        }
        self.proxy_url = proxy_url
            .map(|proxy_url| zenith_relay_core::normalize_proxy_url(&proxy_url))
            .transpose()
            .map_err(|_| {
                CredentialError::new(
                    CredentialErrorCode::InvalidSecret,
                    "account proxy URL is invalid",
                )
            })?;
        self.bypass_common_proxy = bypass_common_proxy;
        Ok(self)
    }

    pub fn with_proxy_url(self, proxy_url: Option<String>) -> Result<Self, CredentialError> {
        self.with_proxy_route(proxy_url, false)
    }
}

impl StoredCodexCredentials {
    /// Compares a complete persisted credential snapshot without formatting or
    /// exposing any sensitive token material. Rollback paths use this as a
    /// compare-and-restore guard so they cannot overwrite a later credential
    /// rotation for the same local account.
    pub fn matches_snapshot(&self, other: &Self) -> bool {
        self.version == other.version
            && self.oauth_client_kind == other.oauth_client_kind
            && self.local_account_id == other.local_account_id
            && self.access_token == other.access_token
            && self.refresh_token == other.refresh_token
            && self.id_token == other.id_token
            && self.expires_at_ms == other.expires_at_ms
            && self.issued_at_ms == other.issued_at_ms
            && self.generation == other.generation
            && self.email == other.email
            && self.phone == other.phone
            && self.password == other.password
            && self.totp_secret == other.totp_secret
            && self.provider_account_id == other.provider_account_id
            && self.provider_user_id == other.provider_user_id
            && self.organization_id == other.organization_id
            && self.plan_type == other.plan_type
            && self.account_is_fedramp == other.account_is_fedramp
            && self.proxy_url == other.proxy_url
            && self.bypass_common_proxy == other.bypass_common_proxy
            && self.basis_points_headers == other.basis_points_headers
            && match (&self.agent_identity, &other.agent_identity) {
                (None, None) => true,
                (Some(left), Some(right)) => {
                    left.private_key() == right.private_key()
                        && left.runtime_id() == right.runtime_id()
                        && left.task_id() == right.task_id()
                }
                _ => false,
            }
    }

    pub fn snapshots_match(stored: Option<&Self>, expected: Option<&Self>) -> bool {
        match (stored, expected) {
            (Some(stored), Some(expected)) => stored.matches_snapshot(expected),
            (None, None) => true,
            _ => false,
        }
    }

    pub fn snapshot(&self) -> StoredCredentialSnapshot {
        StoredCredentialSnapshot {
            version: self.version,
            oauth_client_kind: self.oauth_client_kind,
            local_account_id: self.local_account_id.clone(),
            identity: self.email.as_deref().map(mask_email),
            has_refresh_token: self.refresh_token.is_some(),
            has_id_token: self.id_token.is_some(),
            has_provider_account_id: self.provider_account_id.is_some(),
            expires_at_ms: self.expires_at_ms,
            issued_at_ms: self.issued_at_ms,
            generation: self.generation,
            plan_type: self.plan_type.clone(),
            account_is_fedramp: self.account_is_fedramp,
            proxy_configured: self.proxy_url.is_some(),
            agent_identity: self.agent_identity.is_some(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCredentialSnapshot {
    pub version: u32,
    pub oauth_client_kind: OAuthClientKind,
    pub local_account_id: String,
    pub identity: Option<String>,
    pub has_refresh_token: bool,
    pub has_id_token: bool,
    pub has_provider_account_id: bool,
    pub expires_at_ms: Option<u64>,
    pub issued_at_ms: u64,
    pub generation: u64,
    pub plan_type: Option<String>,
    pub account_is_fedramp: bool,
    pub proxy_configured: bool,
    pub agent_identity: bool,
}
