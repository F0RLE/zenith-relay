use super::super::{OAuthTokenSet, parse::validate_token};
use super::{OAuthError, OAuthErrorCode, SiwcRegistration};
use serde::{Deserialize, Serialize};

/// Received token material, not an authorization. Only the protected rotation
/// checkpoint may persist it before the ID token is verified.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PendingSiwcRefresh {
    registration: SiwcRegistration,
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    expires_at_ms: u64,
    received_at_ms: u64,
}

impl PendingSiwcRefresh {
    /// Only for ending the received session; this does not authorize inference.
    pub(in crate::local_pool::accounts) fn revocation_token(&self) -> &str {
        &self.refresh_token
    }

    pub(super) fn new(
        tokens: OAuthTokenSet,
        registration: SiwcRegistration,
        now_ms: u64,
    ) -> Result<Self, OAuthError> {
        let invalid = || OAuthError::new(OAuthErrorCode::InvalidResponse, false);
        let (access_token, refresh_token, id_token, expires_at_ms) = tokens.into_secret_parts();
        let pending = Self {
            registration,
            access_token,
            refresh_token: refresh_token.ok_or_else(invalid)?,
            id_token,
            expires_at_ms: expires_at_ms.ok_or_else(invalid)?,
            received_at_ms: now_ms,
        };
        pending.validate_for(&pending.registration)?;
        Ok(pending)
    }

    pub fn validate_for(&self, before: &SiwcRegistration) -> Result<(), OAuthError> {
        let invalid = || OAuthError::new(OAuthErrorCode::InvalidResponse, false);
        before.validate().map_err(|_| invalid())?;
        self.registration.validate().map_err(|_| invalid())?;
        if before.requires_reauthentication
            || self.registration.requires_reauthentication
            || before.client_id != self.registration.client_id
            || before.host_id != self.registration.host_id
            || before.subject != self.registration.subject
            || before.issuer != self.registration.issuer
            || self.expires_at_ms <= self.received_at_ms
        {
            return Err(invalid());
        }
        validate_token(&self.access_token)?;
        validate_token(&self.refresh_token)?;
        if let Some(id_token) = &self.id_token {
            validate_token(id_token)?;
        }
        Ok(())
    }

    pub(super) fn into_tokens(self) -> Result<(OAuthTokenSet, SiwcRegistration), OAuthError> {
        self.validate_for(&self.registration)?;
        Ok((
            OAuthTokenSet {
                access_token: self.access_token,
                refresh_token: Some(self.refresh_token),
                id_token: self.id_token,
                expires_at_ms: Some(self.expires_at_ms),
                verified_identity: None,
                siwc: None,
            },
            self.registration,
        ))
    }
}

impl std::fmt::Debug for PendingSiwcRefresh {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingSiwcRefresh")
            .finish_non_exhaustive()
    }
}
