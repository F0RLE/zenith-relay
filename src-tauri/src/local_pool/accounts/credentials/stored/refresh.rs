use super::super::super::oauth::OAuthTokenSet;
use super::super::error::CredentialError;
use super::super::wire::{validate_optional, validate_token, MAX_TOKEN_BYTES};
use std::fmt;
use zenith_relay_core::omit_blank;

pub struct CredentialRefresh {
    pub(in crate::local_pool::accounts::credentials::stored) access_token: String,
    pub(in crate::local_pool::accounts::credentials::stored) refresh_token: Option<String>,
    pub(in crate::local_pool::accounts::credentials::stored) id_token: Option<String>,
    pub(in crate::local_pool::accounts::credentials::stored) expires_at_ms: Option<u64>,
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
            refresh_token: omit_blank(refresh_token),
            id_token: omit_blank(id_token),
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
