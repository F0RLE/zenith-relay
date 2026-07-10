use super::{import_session::SecretBackend, oauth::OAuthTokenSet};
use serde::{Deserialize, Serialize};
use std::{fmt, sync::Arc};
use zenith_relay_core::accounts::{TokenRefresh, TokenSet};

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
        })
    }

    pub fn local_account_id(&self) -> &str {
        &self.local_account_id
    }

    pub fn access_token(&self) -> &str {
        &self.access_token
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    #[cfg(test)]
    pub fn id_token(&self) -> Option<&str> {
        self.id_token.as_deref()
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
                "stored Codex token set is invalid",
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
                "stored Codex token refresh is invalid",
            )
        })
    }

    pub fn with_token_set(&self, tokens: &TokenSet) -> Result<Self, CredentialError> {
        Self::new(
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
        )
    }

    pub fn apply_refresh(
        &self,
        refresh: CredentialRefresh,
        issued_at_ms: u64,
    ) -> Result<Self, CredentialError> {
        Self::new(
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
        )
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
        }
    }

    fn to_secret_json(&self) -> Result<String, CredentialError> {
        let wire = CredentialWire::from(self);
        let value = serde_json::to_string(&wire).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "failed to encode stored Codex credentials",
            )
        })?;
        if value.len() > MAX_SECRET_JSON_BYTES {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored Codex credentials exceed the size limit",
            ));
        }
        Ok(value)
    }

    fn from_secret_json(value: &str) -> Result<Self, CredentialError> {
        if value.is_empty() || value.len() > MAX_SECRET_JSON_BYTES {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored Codex credentials are invalid",
            ));
        }
        let wire: CredentialWire = serde_json::from_str(value).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "stored Codex credentials are invalid",
            )
        })?;
        if wire.version != CREDENTIAL_VERSION {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidVersion,
                "stored Codex credential version is unsupported",
            ));
        }
        Self::new(
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
        )
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
                "failed to save Codex credentials",
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
                "failed to load Codex credentials",
            )
        })?
        else {
            return Ok(None);
        };
        let credentials = StoredCodexCredentials::from_secret_json(&value)?;
        if credentials.local_account_id() != local_account_id {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidIdentity,
                "stored Codex credential identity does not match",
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
                "stored Codex credentials are missing",
            )
        })
    }

    pub fn delete(&self, local_account_id: &str) -> Result<(), CredentialError> {
        let secret_ref = credential_secret_ref(local_account_id)?;
        self.backend.delete(&secret_ref).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::SecretStoreUnavailable,
                "failed to delete Codex credentials",
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
            "stored Codex token is invalid",
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
            "stored Codex credential metadata is invalid",
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
        for secret in [ACCESS, REFRESH, ID_TOKEN, EMAIL, PROVIDER_ACCOUNT] {
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
        assert_eq!(updated.generation(), 8);
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
    }
}
