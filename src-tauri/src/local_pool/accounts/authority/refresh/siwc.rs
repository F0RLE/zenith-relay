use super::*;
use crate::local_pool::accounts::credentials::StoredCodexCredentials;

impl<B, C> StoredRefreshAdapter<B, C>
where
    B: SecretBackend + Send + Sync,
    C: CodexRefreshClient,
{
    pub(super) async fn receive_and_verify_siwc(
        &self,
        before: &StoredCodexCredentials,
        pending: Option<PendingSiwcRefresh>,
        now_ms: u64,
        revision: Option<&TokenDispatchRevision>,
    ) -> Result<CredentialRefresh, TokenRefreshFailure> {
        let superseded = || {
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "credential_superseded")
        };
        let persist_failed = || {
            TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                error_codes::CREDENTIAL_PERSIST_FAILED,
            )
        };
        let received = match pending {
            Some(pending) => Ok(pending),
            None => {
                self.client
                    .refresh_siwc(
                        before.local_account_id(),
                        before.siwc_registration().ok_or_else(superseded)?,
                        before.refresh_token().ok_or_else(superseded)?,
                        now_ms,
                    )
                    .await
            }
        };
        // A replacement login/removal wins over both the response and its checkpoint.
        // No guard is held across network or signature verification awaits.
        {
            let _revision_guard = revision
                .map(|revision| revision.guard().ok_or_else(superseded))
                .transpose()?;
            let current = self
                .credentials
                .require(before.local_account_id())
                .map_err(|_| persist_failed())?;
            if !current.matches_siwc_grant(before) {
                return Err(superseded());
            }
            match &received {
                Ok(pending) => self
                    .credentials
                    .checkpoint_pending_siwc_rotation(&current, pending)
                    .map_err(|_| persist_failed())?,
                Err(error) if siwc_refresh_definitely_rejected(&error.code) => {
                    self.credentials
                        .clear_token_rotation(before.local_account_id())
                        .map_err(|_| persist_failed())?;
                }
                Err(_) => {}
            }
        }
        // Restart/cancellation/JWKS outages resume verification of this exact reply.
        // Unverified material never enters TokenAuthority or the primary credentials.
        self.client
            .verify_siwc_refresh(before.local_account_id(), received?, now_ms)
            .await
    }
}
