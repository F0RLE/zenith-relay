use super::super::import_session::SecretBackend;
use super::error::{CredentialError, CredentialErrorCode};
use super::stored::StoredCodexCredentials;
use super::wire::validate_local_account_id;
use std::sync::Arc;

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
