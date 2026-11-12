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
        let mut received = match pending {
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
        if let Err(error) = &mut received {
            mark_uncertain_exchange(error);
        }
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

fn mark_uncertain_exchange(failure: &mut TokenRefreshFailure) {
    if failure.kind == TokenRefreshFailureKind::Transient
        && !siwc_refresh_definitely_rejected(&failure.code)
    {
        failure.kind = TokenRefreshFailureKind::RotationOutcomeUnknown;
    }
}

pub(super) fn record_renewal_failure(account_id: &str, failure: &TokenRefreshFailure) {
    let details = [
        ("account", crate::diagnostics::hash_identifier(account_id)),
        ("code", failure.code.clone()),
        (
            "stage",
            failure
                .stage
                .map_or("unknown", |stage| stage.as_str())
                .into(),
        ),
        (
            "request_id",
            failure
                .request_id
                .clone()
                .unwrap_or_else(|| "unreported".into()),
        ),
        (
            "next_retry_at_ms",
            failure
                .retry_at_ms
                .map_or_else(|| "unreported".into(), |at| at.to_string()),
        ),
    ];
    if failure.kind == TokenRefreshFailureKind::Deferred {
        crate::diagnostics::record_operation("siwc-token-renewal", "scheduled", &details);
    } else {
        crate::diagnostics::record_error(
            "siwc-token-renewal",
            Some(&failure.code),
            "SIWC token renewal could not finish",
            &details,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exchange_uncertainty_preserves_typed_auth_failures_and_scheduling() {
        for kind in [
            TokenRefreshFailureKind::InvalidGrant,
            TokenRefreshFailureKind::ReusedRefreshToken,
            TokenRefreshFailureKind::ExpiredRefreshToken,
            TokenRefreshFailureKind::InvalidatedRefreshToken,
            TokenRefreshFailureKind::RotationRecoveryRequired,
            TokenRefreshFailureKind::Deferred,
        ] {
            let mut failure = TokenRefreshFailure::new(kind, "synthetic_provider_code");
            mark_uncertain_exchange(&mut failure);
            assert_eq!(failure.kind, kind);
        }
        let mut rejected =
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_client");
        mark_uncertain_exchange(&mut rejected);
        assert_eq!(rejected.kind, TokenRefreshFailureKind::Transient);
        let mut lost = TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "transport")
            .with_stage(zenith_relay_core::accounts::TokenRefreshStage::ResponseRead)
            .with_request_id(Some("req_synthetic-response"));
        mark_uncertain_exchange(&mut lost);
        assert_eq!(lost.kind, TokenRefreshFailureKind::RotationOutcomeUnknown);
        assert_eq!(
            lost.stage,
            Some(zenith_relay_core::accounts::TokenRefreshStage::ResponseRead)
        );
        assert_eq!(lost.request_id.as_deref(), Some("req_synthetic-response"));
    }
}
