use super::super::error::{CredentialError, CredentialErrorCode};
use super::{CredentialRefresh, StoredCodexCredentials};
use zenith_relay_core::accounts::{TokenRefresh, TokenSet};

impl StoredCodexCredentials {
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
        self.rebuild_with_token_material(
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
        )
    }

    pub fn apply_refresh(
        &self,
        refresh: CredentialRefresh,
        issued_at_ms: u64,
    ) -> Result<Self, CredentialError> {
        let CredentialRefresh {
            access_token,
            refresh_token,
            id_token,
            expires_at_ms,
        } = refresh;
        self.rebuild_with_token_material(
            access_token,
            refresh_token.or_else(|| self.refresh_token.clone()),
            id_token.or_else(|| self.id_token.clone()),
            expires_at_ms,
            issued_at_ms,
            self.generation.saturating_add(1),
        )
    }

    fn rebuild_with_token_material(
        &self,
        access_token: String,
        refresh_token: Option<String>,
        id_token: Option<String>,
        expires_at_ms: Option<u64>,
        issued_at_ms: u64,
        generation: u64,
    ) -> Result<Self, CredentialError> {
        let mut updated = Self::new(
            &self.local_account_id,
            access_token,
            refresh_token,
            id_token,
            expires_at_ms,
            issued_at_ms,
            generation,
            self.email.clone(),
            self.provider_account_id.clone(),
            self.provider_user_id.clone(),
            self.organization_id.clone(),
            self.plan_type.clone(),
            self.account_is_fedramp,
        )?
        .with_oauth_client_kind(self.oauth_client_kind)
        .with_proxy_route(self.proxy_url.clone(), self.bypass_common_proxy)?;
        updated.agent_identity = self.agent_identity.clone();
        updated.basis_points_headers = self.basis_points_headers.clone();
        updated.phone = self.phone.clone();
        updated.password = self.password.clone();
        updated.totp_secret = self.totp_secret.clone();
        Ok(updated)
    }
}
