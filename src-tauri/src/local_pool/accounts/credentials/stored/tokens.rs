use super::super::error::{CredentialError, CredentialErrorCode};
use super::{CredentialRefresh, StoredCodexCredentials};
use zenith_relay_core::accounts::{TokenRefresh, TokenSet};

impl StoredCodexCredentials {
    pub(in crate::local_pool::accounts) fn refresh_is_paused(&self, now_ms: u64) -> bool {
        self.refresh_retry.is_some_and(|retry| {
            let delay = 5_000_u64
                .saturating_mul(1_u64 << retry.failures.saturating_sub(1).min(6))
                .min(300_000);
            // A backwards wall-clock correction must not lock an account indefinitely.
            now_ms.saturating_add(300_000) >= retry.failed_at_ms
                && now_ms < retry.failed_at_ms.saturating_add(delay)
        })
    }

    pub(in crate::local_pool::accounts) fn with_refresh_failure(mut self, now_ms: u64) -> Self {
        let failures = self
            .refresh_retry
            .filter(|retry| now_ms >= retry.failed_at_ms)
            .map_or(1, |retry| retry.failures.saturating_add(1).min(7));
        self.refresh_retry = Some(super::RefreshRetry {
            failures,
            failed_at_ms: now_ms,
        });
        self
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
            siwc,
            access_token,
            refresh_token,
            id_token,
            expires_at_ms,
        } = refresh;
        if self
            .siwc
            .as_ref()
            .map(|r| (&r.client_id, &r.subject, &r.host_id))
            != siwc
                .as_ref()
                .map(|r| (&r.client_id, &r.subject, &r.host_id))
        {
            return Err(CredentialError::new(
                CredentialErrorCode::InvalidSecret,
                "SIWC refresh changed registration identity",
            ));
        }
        self.rebuild_with_token_material(
            access_token,
            refresh_token.or_else(|| self.refresh_token.clone()),
            id_token.or_else(|| self.id_token.clone()),
            expires_at_ms,
            issued_at_ms,
            self.generation.saturating_add(1),
        )
        .and_then(|credentials| credentials.with_siwc_registration(siwc))
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
        updated.siwc = self.siwc.clone();
        updated.basis_points_headers = self.basis_points_headers.clone();
        updated.phone = self.phone.clone();
        updated.password = self.password.clone();
        updated.totp_secret = self.totp_secret.clone();
        Ok(updated)
    }
}
