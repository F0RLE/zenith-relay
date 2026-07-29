use super::{import_session::SecretBackend, oauth::OAuthTokenSet};
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};
use zenith_relay_core::accounts::{TokenRefresh, TokenSet};
use zenith_relay_core::providers::chatgpt::AgentIdentityCredential;

const CREDENTIAL_VERSION: u32 = 1;
const MAX_SECRET_JSON_BYTES: usize = 256 * 1024;
const MAX_TOKEN_BYTES: usize = 64 * 1024;
const MAX_ID_BYTES: usize = 256;
const MAX_EMAIL_BYTES: usize = 320;
const MAX_PLAN_BYTES: usize = 64;

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
    fn new(code: CredentialErrorCode, message: &'static str) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl fmt::Display for CredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CredentialError {}

#[derive(Clone)]
pub struct StoredCodexCredentials {
    version: u32,
    local_account_id: String,
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
    proxy_url: Option<String>,
    bypass_common_proxy: bool,
    agent_identity: Option<AgentIdentityCredential>,
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
            local_account_id: local_account_id.to_string(),
            access_token,
            refresh_token: nonempty(refresh_token),
            id_token: nonempty(id_token),
            expires_at_ms,
            issued_at_ms,
            generation,
            email: nonempty(email),
            provider_account_id: nonempty(provider_account_id),
            provider_user_id: nonempty(provider_user_id),
            organization_id: nonempty(organization_id),
            plan_type: nonempty(plan_type),
            account_is_fedramp,
            proxy_url: None,
            bypass_common_proxy: false,
            agent_identity: None,
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
            local_account_id: local_account_id.to_string(),
            access_token: String::new(),
            refresh_token: None,
            id_token: None,
            expires_at_ms: None,
            issued_at_ms,
            generation,
            email: nonempty(email),
            provider_account_id: nonempty(provider_account_id),
            provider_user_id: nonempty(provider_user_id),
            organization_id: nonempty(organization_id),
            plan_type: nonempty(plan_type),
            account_is_fedramp,
            proxy_url: None,
            bypass_common_proxy: false,
            agent_identity: Some(agent_identity),
        })
    }

    pub fn local_account_id(&self) -> &str {
        &self.local_account_id
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
        let mut authorization = HeaderValue::from_str(&format!("Bearer {}", self.access_token))
            .map_err(|_| {
                CredentialError::new(
                    CredentialErrorCode::InvalidSecret,
                    "stored ChatGPT token is invalid",
                )
            })?;
        authorization.set_sensitive(true);
        Ok(authorization)
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

    pub fn plan_type(&self) -> Option<&str> {
        self.plan_type.as_deref()
    }

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
            .map(|value| zenith_relay_core::normalize_proxy_url(&value))
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

    pub fn expire_access_at(&mut self, now_ms: u64) {
        self.expires_at_ms = Some(now_ms);
    }

    pub fn is_access_usable(&self, now_ms: u64, refresh_skew_ms: u64) -> bool {
        self.expires_at_ms
            .is_none_or(|expires_at| expires_at > now_ms.saturating_add(refresh_skew_ms))
    }

    pub fn to_token_set(&self) -> Result<TokenSet, CredentialError> {
        TokenSet::new(
            self.access_token.clone(),
            self.refresh_token.clone(),
            self.id_token.clone(),
            self.expires_at_ms,
            self.issued_at_ms,
            self.generation,
        )
        .map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored ChatGPT token set is invalid",
            )
        })
    }

    pub fn to_token_refresh(&self) -> Result<TokenRefresh, CredentialError> {
        TokenRefresh::new(
            self.access_token.clone(),
            self.refresh_token.clone(),
            self.id_token.clone(),
            self.expires_at_ms,
        )
        .map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored ChatGPT token refresh is invalid",
            )
        })
    }

    pub fn with_token_set(&self, tokens: &TokenSet) -> Result<Self, CredentialError> {
        let mut updated = Self::new(
            &self.local_account_id,
            tokens.access_token().to_string(),
            tokens
                .refresh_token()
                .map(str::to_string)
                .or_else(|| self.refresh_token.clone()),
            tokens
                .id_token()
                .map(str::to_string)
                .or_else(|| self.id_token.clone()),
            tokens.expires_at_ms(),
            tokens.issued_at_ms(),
            tokens.generation(),
            self.email.clone(),
            self.provider_account_id.clone(),
            self.provider_user_id.clone(),
            self.organization_id.clone(),
            self.plan_type.clone(),
            self.account_is_fedramp,
        )?
        .with_proxy_route(self.proxy_url.clone(), self.bypass_common_proxy)?;
        updated.agent_identity = self.agent_identity.clone();
        Ok(updated)
    }

    pub fn apply_refresh(
        &self,
        refresh: CredentialRefresh,
        issued_at_ms: u64,
    ) -> Result<Self, CredentialError> {
        let mut updated = Self::new(
            &self.local_account_id,
            refresh.access_token,
            refresh.refresh_token.or_else(|| self.refresh_token.clone()),
            refresh.id_token.or_else(|| self.id_token.clone()),
            refresh.expires_at_ms,
            issued_at_ms,
            self.generation.saturating_add(1),
            self.email.clone(),
            self.provider_account_id.clone(),
            self.provider_user_id.clone(),
            self.organization_id.clone(),
            self.plan_type.clone(),
            self.account_is_fedramp,
        )?
        .with_proxy_route(self.proxy_url.clone(), self.bypass_common_proxy)?;
        updated.agent_identity = self.agent_identity.clone();
        Ok(updated)
    }

    pub fn snapshot(&self) -> StoredCredentialSnapshot {
        StoredCredentialSnapshot {
            version: self.version,
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

    fn to_secret_json(&self) -> Result<String, CredentialError> {
        let wire = CredentialWire::from(self);
        let value = serde_json::to_string(&wire).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "failed to encode stored ChatGPT credentials",
            )
        })?;
        if value.len() > MAX_SECRET_JSON_BYTES {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored ChatGPT credentials exceed the size limit",
            ));
        }
        Ok(value)
    }

    fn from_secret_json(value: &str) -> Result<Self, CredentialError> {
        if value.is_empty() || value.len() > MAX_SECRET_JSON_BYTES {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored ChatGPT credentials are invalid",
            ));
        }
        let wire: CredentialWire = serde_json::from_str(value).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored ChatGPT credentials are invalid",
            )
        })?;
        if wire.version != CREDENTIAL_VERSION {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidVersion,
                "stored ChatGPT credential version is unsupported",
            ));
        }
        let agent_identity = wire
            .agent_identity
            .map(|agent| match agent.task_id {
                Some(task_id) => {
                    AgentIdentityCredential::new(agent.private_key, agent.runtime_id, task_id)
                }
                None => AgentIdentityCredential::unregistered(agent.private_key, agent.runtime_id),
            })
            .transpose()
            .map_err(|_| {
                CredentialError::new(
                    CredentialErrorCode::InvalidSecret,
                    "stored Agent Identity credential is invalid",
                )
            })?;
        let credentials = if wire.access_token.is_empty() {
            Self::new_agent_identity(
                &wire.local_account_id,
                agent_identity.ok_or_else(|| {
                    CredentialError::new(
                        CredentialErrorCode::InvalidSecret,
                        "stored ChatGPT credential has no authorization method",
                    )
                })?,
                wire.issued_at_ms,
                wire.generation,
                wire.email,
                wire.provider_account_id,
                wire.provider_user_id,
                wire.organization_id,
                wire.plan_type,
                wire.account_is_fedramp,
            )?
        } else {
            let credentials = Self::new(
                &wire.local_account_id,
                wire.access_token,
                wire.refresh_token,
                wire.id_token,
                wire.expires_at_ms,
                wire.issued_at_ms,
                wire.generation,
                wire.email,
                wire.provider_account_id,
                wire.provider_user_id,
                wire.organization_id,
                wire.plan_type,
                wire.account_is_fedramp,
            )?;
            match agent_identity {
                Some(agent_identity) => credentials.with_agent_identity(agent_identity),
                None => credentials,
            }
        };
        credentials.with_proxy_route(wire.proxy_url, wire.bypass_common_proxy)
    }
}

impl fmt::Debug for StoredCodexCredentials {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StoredCodexCredentials")
            .field("version", &self.version)
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
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredCredentialSnapshot {
    pub version: u32,
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

pub struct CredentialRefresh {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_at_ms: Option<u64>,
}

impl CredentialRefresh {
    pub fn new(
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_at_ms: Option<u64>,
    ) -> Result<Self, CredentialError> {
        validate_token(&access_token)?;
        validate_optional(refresh_token.as_deref(), MAX_TOKEN_BYTES)?;
        validate_optional(id_token.as_deref(), MAX_TOKEN_BYTES)?;
        Ok(Self {
            access_token,
            refresh_token: nonempty(refresh_token),
            id_token: nonempty(id_token),
            expires_at_ms,
        })
    }

    pub fn from_oauth(tokens: OAuthTokenSet) -> Result<Self, CredentialError> {
        let (access_token, refresh_token, id_token, expires_at_ms) = tokens.into_secret_parts();
        Self::new(access_token, refresh_token, id_token, expires_at_ms)
    }
}

impl fmt::Debug for CredentialRefresh {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialRefresh")
            .field("access_token", &"[redacted]")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "[redacted]"),
            )
            .field("id_token", &self.id_token.as_ref().map(|_| "[redacted]"))
            .field("expires_at_ms", &self.expires_at_ms)
            .finish()
    }
}

pub struct CredentialStore<B> {
    backend: Arc<B>,
}

impl<B> Clone for CredentialStore<B> {
    fn clone(&self) -> Self {
        Self {
            backend: self.backend.clone(),
        }
    }
}

impl<B: SecretBackend> CredentialStore<B> {
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }

    pub fn from_backend(backend: B) -> Self {
        Self::new(Arc::new(backend))
    }

    pub fn save(&self, credentials: &StoredCodexCredentials) -> Result<(), CredentialError> {
        let secret_ref = credential_secret_ref(credentials.local_account_id())?;
        let value = credentials.to_secret_json()?;
        self.backend.save(&secret_ref, &value).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::SecretStoreUnavailable,
                "failed to save ChatGPT credentials",
            )
        })
    }

    pub fn load(
        &self,
        local_account_id: &str,
    ) -> Result<Option<StoredCodexCredentials>, CredentialError> {
        let secret_ref = credential_secret_ref(local_account_id)?;
        let Some(value) = self.backend.load(&secret_ref).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::SecretStoreUnavailable,
                "failed to load ChatGPT credentials",
            )
        })?
        else {
            return Ok(None);
        };
        let credentials = StoredCodexCredentials::from_secret_json(&value)?;
        if credentials.local_account_id() != local_account_id {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidIdentity,
                "stored ChatGPT credential identity does not match",
            ));
        }
        Ok(Some(credentials))
    }

    pub fn require(
        &self,
        local_account_id: &str,
    ) -> Result<StoredCodexCredentials, CredentialError> {
        self.load(local_account_id)?.ok_or_else(|| {
            CredentialError::new(
                CredentialErrorCode::SecretMissing,
                "stored ChatGPT credentials are missing",
            )
        })
    }

    pub fn delete(&self, local_account_id: &str) -> Result<(), CredentialError> {
        let secret_ref = credential_secret_ref(local_account_id)?;
        self.backend.delete(&secret_ref).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::SecretStoreUnavailable,
                "failed to delete ChatGPT credentials",
            )
        })
    }
}

pub fn credential_secret_ref(local_account_id: &str) -> Result<String, CredentialError> {
    validate_local_account_id(local_account_id)?;
    Ok(format!("account:codex:{local_account_id}"))
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialWire {
    version: u32,
    local_account_id: String,
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
    #[serde(default)]
    proxy_url: Option<String>,
    #[serde(default)]
    bypass_common_proxy: bool,
    #[serde(default)]
    agent_identity: Option<AgentIdentityWire>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AgentIdentityWire {
    private_key: String,
    runtime_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
}

impl From<&StoredCodexCredentials> for CredentialWire {
    fn from(value: &StoredCodexCredentials) -> Self {
        Self {
            version: value.version,
            local_account_id: value.local_account_id.clone(),
            access_token: value.access_token.clone(),
            refresh_token: value.refresh_token.clone(),
            id_token: value.id_token.clone(),
            expires_at_ms: value.expires_at_ms,
            issued_at_ms: value.issued_at_ms,
            generation: value.generation,
            email: value.email.clone(),
            provider_account_id: value.provider_account_id.clone(),
            provider_user_id: value.provider_user_id.clone(),
            organization_id: value.organization_id.clone(),
            plan_type: value.plan_type.clone(),
            account_is_fedramp: value.account_is_fedramp,
            proxy_url: value.proxy_url.clone(),
            bypass_common_proxy: value.bypass_common_proxy,
            agent_identity: value
                .agent_identity
                .as_ref()
                .map(|agent| AgentIdentityWire {
                    private_key: agent.private_key().to_string(),
                    runtime_id: agent.runtime_id().to_string(),
                    task_id: agent.task_id().map(str::to_string),
                }),
        }
    }
}

fn validate_local_account_id(value: &str) -> Result<(), CredentialError> {
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

fn validate_token(value: &str) -> Result<(), CredentialError> {
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

fn validate_optional(value: Option<&str>, max_bytes: usize) -> Result<(), CredentialError> {
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

fn nonempty(value: Option<String>) -> Option<String> {
    value.filter(|value| !value.trim().is_empty())
}

fn mask_email(value: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::super::import_session::SecretBackendError;
    use super::*;
    use std::{collections::HashMap, sync::Mutex};

    const ACCESS: &str = "stored-access-secret";
    const REFRESH: &str = "stored-refresh-secret";
    const ID_TOKEN: &str = "stored-id-secret";
    const EMAIL: &str = "stored.user@example.test";
    const PROVIDER_ACCOUNT: &str = "provider-account-secret-id";
    const PROXY: &str = "http://proxy-user:proxy-pass@proxy.example:8080/";

    #[derive(Default)]
    struct MemorySecrets(Mutex<HashMap<String, String>>);

    impl SecretBackend for MemorySecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<(), SecretBackendError> {
            self.0
                .lock()
                .unwrap()
                .insert(secret_ref.into(), value.into());
            Ok(())
        }

        fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
            Ok(self.0.lock().unwrap().get(secret_ref).cloned())
        }

        fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
            self.0.lock().unwrap().remove(secret_ref);
            Ok(())
        }
    }

    #[test]
    fn debug_and_snapshot_redact_tokens_email_and_provider_ids() {
        let credentials = fixture();
        let debug = format!("{credentials:?}");
        let snapshot = serde_json::to_string(&credentials.snapshot()).unwrap();
        for secret in [ACCESS, REFRESH, ID_TOKEN, EMAIL, PROVIDER_ACCOUNT, PROXY] {
            assert!(!debug.contains(secret));
            assert!(!snapshot.contains(secret));
        }
        assert!(debug.contains("[redacted]"));
        assert!(snapshot.contains("s***@e***.test"));
    }

    #[test]
    fn native_secret_json_round_trips_after_restart() {
        let backend = Arc::new(MemorySecrets::default());
        let first = CredentialStore::new(backend.clone());
        first.save(&fixture()).unwrap();
        drop(first);

        let reopened = CredentialStore::new(backend);
        let loaded = reopened.require("relay_account_1").unwrap();
        assert_eq!(loaded.access_token(), ACCESS);
        assert_eq!(loaded.refresh_token(), Some(REFRESH));
        assert_eq!(loaded.id_token(), Some(ID_TOKEN));
        assert_eq!(loaded.provider_account_id(), Some(PROVIDER_ACCOUNT));
        assert_eq!(loaded.proxy_url(), Some(PROXY));
        assert_eq!(loaded.generation(), 7);
        assert_eq!(
            credential_secret_ref("relay_account_1").unwrap(),
            "account:codex:relay_account_1"
        );
    }

    #[test]
    fn token_set_merge_preserves_private_identity_metadata() {
        let credentials = fixture();
        let tokens = TokenSet::new("new-access", None, None, Some(9_000), 2_000, 8).unwrap();
        let updated = credentials.with_token_set(&tokens).unwrap();
        assert_eq!(updated.access_token(), "new-access");
        assert_eq!(updated.refresh_token(), Some(REFRESH));
        assert_eq!(updated.id_token(), Some(ID_TOKEN));
        assert_eq!(updated.provider_account_id(), Some(PROVIDER_ACCOUNT));
        assert_eq!(updated.proxy_url(), Some(PROXY));
        assert_eq!(updated.generation(), 8);
    }

    #[test]
    fn direct_route_survives_storage_and_token_refresh() {
        let backend = Arc::new(MemorySecrets::default());
        let store = CredentialStore::new(backend);
        let direct = fixture().with_proxy_route(None, true).unwrap();
        store.save(&direct).unwrap();

        let loaded = store.require("relay_account_1").unwrap();
        assert!(loaded.bypass_common_proxy());
        let tokens = TokenSet::new("new-access", None, None, Some(9_000), 2_000, 8).unwrap();
        assert!(loaded
            .with_token_set(&tokens)
            .unwrap()
            .bypass_common_proxy());
    }

    #[test]
    fn agent_identity_round_trips_without_becoming_an_oauth_token() {
        const PRIVATE_KEY: &str =
            "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
        let backend = Arc::new(MemorySecrets::default());
        let store = CredentialStore::new(backend);
        let credential = StoredCodexCredentials::new_agent_identity(
            "agent_account",
            AgentIdentityCredential::new(
                PRIVATE_KEY.into(),
                "runtime-test".into(),
                "task-test".into(),
            )
            .unwrap(),
            1,
            2,
            Some("agent@example.test".into()),
            Some("provider-agent".into()),
            None,
            None,
            Some("team".into()),
            false,
        )
        .unwrap();
        store.save(&credential).unwrap();

        let loaded = store.require("agent_account").unwrap();
        assert!(loaded.is_agent_identity());
        assert!(loaded.access_token().is_empty());
        assert!(loaded.to_token_set().is_err());
        assert!(loaded
            .authorization(1_785_000_000_000)
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("AgentAssertion "));
        assert!(!format!("{loaded:?}").contains(PRIVATE_KEY));
    }

    #[test]
    fn oauth_fallback_round_trips_and_survives_token_refresh_with_agent_identity() {
        const PRIVATE_KEY: &str =
            "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
        let backend = Arc::new(MemorySecrets::default());
        let store = CredentialStore::new(backend);
        let credentials = fixture().with_agent_identity(
            AgentIdentityCredential::new(
                PRIVATE_KEY.into(),
                "runtime-test".into(),
                "task-test".into(),
            )
            .unwrap(),
        );
        store.save(&credentials).unwrap();

        let loaded = store.require("relay_account_1").unwrap();
        assert!(loaded.is_agent_identity());
        assert!(loaded.has_oauth());
        assert_eq!(loaded.to_token_set().unwrap().access_token(), ACCESS);
        let refreshed = loaded
            .with_token_set(
                &TokenSet::new("new-access", None, None, Some(9_000), 2_000, 8).unwrap(),
            )
            .unwrap();
        assert_eq!(refreshed.access_token(), "new-access");
        assert_eq!(
            refreshed.agent_identity().unwrap().task_id(),
            Some("task-test")
        );
    }

    fn fixture() -> StoredCodexCredentials {
        StoredCodexCredentials::new(
            "relay_account_1",
            ACCESS.into(),
            Some(REFRESH.into()),
            Some(ID_TOKEN.into()),
            Some(1),
            0,
            7,
            Some(EMAIL.into()),
            Some(PROVIDER_ACCOUNT.into()),
            Some("provider-user-secret-id".into()),
            Some("provider-org-secret-id".into()),
            Some("plus".into()),
            false,
        )
        .unwrap()
        .with_proxy_url(Some(PROXY.into()))
        .unwrap()
    }
}
