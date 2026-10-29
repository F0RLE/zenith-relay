use super::super::import_session::SecretBackend;
use super::error::{CredentialError, CredentialErrorCode};
use super::stored::StoredCodexCredentials;
use super::wire::validate_local_account_id;
use std::sync::Arc;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TokenRotation {
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
    pub fn checkpoint_token_rotation(
        &self,
        before: &StoredCodexCredentials,
        after: Option<&StoredCodexCredentials>,
    ) -> Result<(), CredentialError> {
        let reference = rotation_secret_ref(before)?;
        let checkpoint = TokenRotation {
            before: before.to_secret_json()?,
            after: after
                .map(StoredCodexCredentials::to_secret_json)
                .transpose()?,
        };
        let encoded = serde_json::to_string(&checkpoint).map_err(|_| rotation_invalid())?;
        self.backend
            .save(&reference, &encoded)
            .map_err(|_| rotation_storage_error())?;
        if self
            .backend
            .load(&reference)
            .map_err(|_| rotation_storage_error())?
            .as_deref()
            != Some(&encoded)
        {
            return Err(rotation_storage_error());
        }
        Ok(())
    }

    pub fn clear_token_rotation(&self, account_id: &str) -> Result<(), CredentialError> {
        let primary = credential_secret_ref(account_id)?;
        for suffix in ["siwc-rotation", "oauth-rotation"] {
            let reference = format!("{primary}:{suffix}");
            if self
                .backend
                .load(&reference)
                .map_err(|_| rotation_storage_error())?
                .is_some()
            {
                self.backend
                    .delete(&reference)
                    .map_err(|_| rotation_storage_error())?;
            }
        }
        Ok(())
    }

    pub fn recover_token_rotation(
        &self,
        current: &StoredCodexCredentials,
    ) -> Result<Option<StoredCodexCredentials>, CredentialError> {
        let reference = rotation_secret_ref(current)?;
        let Some(encoded) = self
            .backend
            .load(&reference)
            .map_err(|_| rotation_storage_error())?
        else {
            return Ok(None);
        };
        if encoded.len() > 2 * super::wire::MAX_SECRET_JSON_BYTES + 4096 {
            return Err(rotation_invalid());
        }
        let checkpoint: TokenRotation =
            serde_json::from_str(&encoded).map_err(|_| rotation_invalid())?;
        let before = StoredCodexCredentials::from_secret_json(&checkpoint.before)
            .map_err(|_| rotation_invalid())?;
        if current.local_account_id() != before.local_account_id() {
            return Err(rotation_invalid());
        }
        if !current.matches_refresh_grant(&before) {
            if current.generation() <= before.generation() {
                return Err(rotation_invalid());
            }
            // A newer login or a completed primary write wins over this checkpoint.
            self.clear_token_rotation(current.local_account_id())?;
            return Ok(None);
        }
        let after = checkpoint.after.ok_or_else(|| CredentialError::new(
            CredentialErrorCode::RotationOutcomeUnknown,
            "token rotation outcome is unknown; sign in again without reusing the old refresh token",
        ))?;
        let after =
            StoredCodexCredentials::from_secret_json(&after).map_err(|_| rotation_invalid())?;
        if after.local_account_id() != current.local_account_id()
            || after.generation() != current.generation().saturating_add(1)
            || after.oauth_client_kind() != current.oauth_client_kind()
            || after.provider_account_id() != current.provider_account_id()
            || after.provider_user_id() != current.provider_user_id()
            || after
                .siwc_registration()
                .map(|r| (&r.client_id, &r.subject, &r.host_id))
                != current
                    .siwc_registration()
                    .map(|r| (&r.client_id, &r.subject, &r.host_id))
        {
            return Err(rotation_invalid());
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
            return Err(rotation_storage_error());
        }
        self.clear_token_rotation(current.local_account_id())?;
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
        self.clear_token_rotation(local_account_id)
    }
}

fn rotation_storage_error() -> CredentialError {
    CredentialError::new(
        CredentialErrorCode::SecretStoreUnavailable,
        "token rotation storage is unavailable; the recovery checkpoint was retained",
    )
}

fn rotation_invalid() -> CredentialError {
    CredentialError::new(
        CredentialErrorCode::RotationCheckpointInvalid,
        "token rotation checkpoint is invalid; sign in again without reusing the old refresh token",
    )
}

fn rotation_secret_ref(credentials: &StoredCodexCredentials) -> Result<String, CredentialError> {
    // Keep existing SIWC checkpoints readable; native OAuth uses its own key.
    let suffix = if credentials.siwc_registration().is_some() {
        "siwc-rotation"
    } else {
        "oauth-rotation"
    };
    Ok(format!(
        "{}:{suffix}",
        credential_secret_ref(credentials.local_account_id())?
    ))
}

pub fn credential_secret_ref(local_account_id: &str) -> Result<String, CredentialError> {
    validate_local_account_id(local_account_id)?;
    Ok(format!("account:codex:{local_account_id}"))
}
