use super::{
    CredentialError, CredentialStore, SecretBackend, StoredCodexCredentials, TokenRotation,
    rotation_invalid,
};
use crate::local_pool::accounts::oauth::siwc::PendingSiwcRefresh;

impl<B: SecretBackend> CredentialStore<B> {
    /// Called under the account credential lock. Reading a replacement for
    /// revocation never promotes unverified credentials into the active store.
    pub fn siwc_revocation_token(
        &self,
        current: &StoredCodexCredentials,
    ) -> Result<Option<String>, CredentialError> {
        let Some(registration) = current.siwc_registration() else {
            return Ok(None);
        };
        if registration.requires_reauthentication || current.is_siwc_identity_only() {
            return Ok(None);
        }
        let token = match self.load_token_rotation(current)? {
            Some(checkpoint) => {
                if let Some(pending) = &checkpoint.pending_siwc {
                    pending
                        .validate_for(registration)
                        .map_err(|_| rotation_invalid())?;
                    Some(pending.revocation_token().to_owned())
                } else {
                    checkpoint
                        .replacement(current)?
                        .refresh_token()
                        .map(str::to_owned)
                }
            }
            None => current.refresh_token().map(str::to_owned),
        };
        token.map(Some).ok_or_else(rotation_invalid)
    }

    pub fn checkpoint_pending_siwc_rotation(
        &self,
        before: &StoredCodexCredentials,
        pending: &PendingSiwcRefresh,
    ) -> Result<(), CredentialError> {
        pending
            .validate_for(before.siwc_registration().ok_or_else(rotation_invalid)?)
            .map_err(|_| rotation_invalid())?;
        self.write_token_rotation(
            before,
            TokenRotation {
                before: before.to_secret_json()?,
                after: None,
                pending_siwc: Some(pending.clone()),
            },
        )
    }

    pub fn pending_siwc_rotation(
        &self,
        current: &StoredCodexCredentials,
    ) -> Result<Option<PendingSiwcRefresh>, CredentialError> {
        let Some(checkpoint) = self.load_token_rotation(current)? else {
            return Ok(None);
        };
        let Some(pending) = checkpoint.pending_siwc else {
            return Ok(None);
        };
        pending
            .validate_for(current.siwc_registration().ok_or_else(rotation_invalid)?)
            .map_err(|_| rotation_invalid())?;
        Ok(Some(pending))
    }
}
