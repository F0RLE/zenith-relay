use super::super::import_session::SecretBackend;
use super::error::{CredentialError, CredentialErrorCode};
use super::stored::StoredCodexCredentials;
use super::wire::validate_local_account_id;
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SiwcRotation {
    before: String,
    after: Option<String>,
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
    pub fn checkpoint_siwc_rotation(
        &self,
        before: &StoredCodexCredentials,
        after: Option<&StoredCodexCredentials>,
    ) -> Result<(), CredentialError> {
        let reference = format!(
            "{}:siwc-rotation",
            credential_secret_ref(before.local_account_id())?
        );
        let checkpoint = SiwcRotation {
            before: before.to_secret_json()?,
            after: after
                .map(StoredCodexCredentials::to_secret_json)
                .transpose()?,
        };
        let encoded = serde_json::to_string(&checkpoint).map_err(|_| rotation_error())?;
        self.backend
            .save(&reference, &encoded)
            .map_err(|_| rotation_error())?;
        if self
            .backend
            .load(&reference)
            .map_err(|_| rotation_error())?
            .as_deref()
            != Some(&encoded)
        {
            return Err(rotation_error());
        }
        Ok(())
    }

    pub fn clear_siwc_rotation(&self, account_id: &str) -> Result<(), CredentialError> {
        let reference = format!("{}:siwc-rotation", credential_secret_ref(account_id)?);
        if self
            .backend
            .load(&reference)
            .map_err(|_| rotation_error())?
            .is_some()
        {
            self.backend
                .delete(&reference)
                .map_err(|_| rotation_error())?;
        }
        Ok(())
    }

    pub fn recover_siwc_rotation(
        &self,
        current: &StoredCodexCredentials,
    ) -> Result<Option<StoredCodexCredentials>, CredentialError> {
        let reference = format!(
            "{}:siwc-rotation",
            credential_secret_ref(current.local_account_id())?
        );
        let Some(encoded) = self
            .backend
            .load(&reference)
            .map_err(|_| rotation_error())?
        else {
            return Ok(None);
        };
        if encoded.len() > 2 * super::wire::MAX_SECRET_JSON_BYTES + 4096 {
            return Err(rotation_error());
        }
        let checkpoint: SiwcRotation =
            serde_json::from_str(&encoded).map_err(|_| rotation_error())?;
        let before = StoredCodexCredentials::from_secret_json(&checkpoint.before)?;
        if current.local_account_id() != before.local_account_id() {
            return Err(rotation_error());
        }
        if !current.matches_siwc_grant(&before) {
            if current.generation() <= before.generation() {
                return Err(rotation_error());
            }
            // A newer login or a completed primary write wins over this checkpoint.
            self.clear_siwc_rotation(current.local_account_id())?;
            return Ok(None);
        }
        let after = checkpoint.after.ok_or_else(rotation_error)?;
        let after = StoredCodexCredentials::from_secret_json(&after)?;
        if after.local_account_id() != current.local_account_id()
            || after.generation() != current.generation().saturating_add(1)
            || after
                .siwc_registration()
                .map(|r| (&r.client_id, &r.subject, &r.host_id))
                != current
                    .siwc_registration()
                    .map(|r| (&r.client_id, &r.subject, &r.host_id))
        {
            return Err(rotation_error());
        }
        // Restore rotated authorization, not stale proxy or account-note settings.
        let after = current
            .with_token_set(&after.to_token_set()?)?
            .with_siwc_registration(after.siwc_registration().cloned())?;
        self.save(&after)?;
        if !self
            .require(current.local_account_id())?
            .matches_snapshot(&after)
        {
            return Err(rotation_error());
        }
        self.clear_siwc_rotation(current.local_account_id())?;
        Ok(Some(after))
    }
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }

    pub fn from_backend(backend: B) -> Self {
        Self::new(Arc::new(backend))
    }

    pub fn save(&self, credentials: &StoredCodexCredentials) -> Result<(), CredentialError> {
        let secret_ref = credential_secret_ref(credentials.local_account_id())?;
        let secret_json = credentials.to_secret_json()?;
        self.backend.save(&secret_ref, &secret_json).map_err(|_| {
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
        let Some(secret_json) = self.backend.load(&secret_ref).map_err(|_| {
            CredentialError::new(
                CredentialErrorCode::SecretStoreUnavailable,
                "failed to load ChatGPT credentials",
            )
        })?
        else {
            return Ok(None);
        };
        let credentials = StoredCodexCredentials::from_secret_json(&secret_json)?;
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
        })?;
        self.clear_siwc_rotation(local_account_id)
    }
}

fn rotation_error() -> CredentialError {
    CredentialError::new(
        CredentialErrorCode::SecretStoreUnavailable,
        "SIWC token rotation needs recovery; the old refresh token was not reused",
    )
}

pub fn credential_secret_ref(local_account_id: &str) -> Result<String, CredentialError> {
    validate_local_account_id(local_account_id)?;
    Ok(format!("account:codex:{local_account_id}"))
}
