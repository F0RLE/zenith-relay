use super::super::{
    credentials::{CredentialRefresh, CredentialStore},
    import_session::SecretBackend,
    oauth::{CodexOAuthClient, OAuthClientKind},
};
use super::lock::{lock_refresh_failure, ProcessAccountLocks, ProcessLockConfig, ProcessLockError};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};
use zenith_relay_core::accounts::{
    TokenDispatchRevision, TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure,
    TokenRefreshFailureKind,
};
use zenith_relay_core::error_codes;

pub trait CodexRefreshClient: Send + Sync {
    fn refresh_siwc<'a>(
        &'a self,
        _local_account_id: &'a str,
        _registration: &'a super::super::oauth::siwc::SiwcRegistration,
        _refresh_token: &'a str,
        _now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async {
            Err(TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                "siwc_refresh_client_missing",
            ))
        })
    }
    fn refresh<'a>(
        &'a self,
        local_account_id: &'a str,
        provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
        kind: OAuthClientKind,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>;
}

impl CodexRefreshClient for CodexOAuthClient {
    fn refresh_siwc<'a>(
        &'a self,
        _local_account_id: &'a str,
        registration: &'a super::super::oauth::siwc::SiwcRegistration,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            let tokens = self
                .exchange_siwc_refresh(registration, refresh_token, now_ms)
                .await?;
            CredentialRefresh::from_oauth(tokens).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_refresh_response",
                )
            })
        })
    }
    fn refresh<'a>(
        &'a self,
        _local_account_id: &'a str,
        _provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
        kind: OAuthClientKind,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            let tokens = self
                .for_kind(kind)
                .exchange_refresh_token(refresh_token, now_ms)
                .await?;
            CredentialRefresh::from_oauth(tokens).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_refresh_response",
                )
            })
        })
    }
}

pub struct StoredRefreshAdapter<B, C> {
    credentials: CredentialStore<B>,
    client: Arc<C>,
    locks: ProcessAccountLocks,
    refresh_skew_ms: u64,
}

impl<B, C> StoredRefreshAdapter<B, C> {
    pub fn new(
        root: PathBuf,
        credentials: CredentialStore<B>,
        client: Arc<C>,
        refresh_skew_ms: u64,
    ) -> Result<Self, ProcessLockError> {
        Self::with_lock_config(
            root,
            credentials,
            client,
            refresh_skew_ms,
            ProcessLockConfig::default(),
        )
    }

    pub fn with_lock_config(
        root: PathBuf,
        credentials: CredentialStore<B>,
        client: Arc<C>,
        refresh_skew_ms: u64,
        config: ProcessLockConfig,
    ) -> Result<Self, ProcessLockError> {
        Ok(Self {
            credentials,
            client,
            locks: ProcessAccountLocks::with_config(root, config)?,
            refresh_skew_ms,
        })
    }
}

impl<B, C> StoredRefreshAdapter<B, C>
where
    B: SecretBackend + Send + Sync,
    C: CodexRefreshClient,
{
    async fn refresh_inner(
        &self,
        local_account_id: &str,
        now_ms: u64,
        revision: Option<&TokenDispatchRevision>,
    ) -> Result<TokenRefresh, TokenRefreshFailure> {
        let _guard = self
            .locks
            .acquire(local_account_id)
            .await
            .map_err(lock_refresh_failure)?;
        let superseded = || {
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "credential_superseded")
        };
        if revision.is_some_and(|revision| revision.guard().is_none()) {
            return Err(superseded());
        }
        let mut stored_credentials = self.credentials.require(local_account_id).map_err(|_| {
            TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                error_codes::CREDENTIAL_LOAD_FAILED,
            )
        })?;
        if stored_credentials.siwc_registration().is_some() {
            let recovered = self
                .credentials
                .recover_siwc_rotation(&stored_credentials)
                .map_err(|_| {
                    TokenRefreshFailure::new(
                        TokenRefreshFailureKind::Transient,
                        "siwc_rotation_recovery_required",
                    )
                })?;
            if let Some(recovered) = recovered {
                stored_credentials = recovered;
            }
        }
        super::super::records::require_plan_permission(&stored_credentials).map_err(|_| {
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "siwc_plan_use_disabled")
        })?;
        if stored_credentials.is_access_usable(now_ms, self.refresh_skew_ms) {
            return stored_credentials.to_token_refresh().map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_stored_credential",
                )
            });
        }
        let refresh_token = stored_credentials.refresh_token().ok_or_else(|| {
            TokenRefreshFailure::new(
                TokenRefreshFailureKind::ExpiredRefreshToken,
                error_codes::REFRESH_TOKEN_MISSING,
            )
        })?;
        let refresh_result = if let Some(registration) = stored_credentials.siwc_registration() {
            self.credentials
                .checkpoint_siwc_rotation(&stored_credentials, None)
                .map_err(|_| {
                    TokenRefreshFailure::new(
                        TokenRefreshFailureKind::Transient,
                        "credential_persist_failed",
                    )
                })?;
            let result = self
                .client
                .refresh_siwc(local_account_id, registration, refresh_token, now_ms)
                .await;
            if let Err(error) = &result {
                // Only an explicit token-endpoint rejection permits reuse. A lost or malformed
                // rotation reply remains fenced until reauthentication or checkpoint recovery.
                if siwc_refresh_definitely_rejected(&error.code) {
                    self.credentials
                        .clear_siwc_rotation(local_account_id)
                        .map_err(|_| {
                            TokenRefreshFailure::new(
                                TokenRefreshFailureKind::Transient,
                                "credential_persist_failed",
                            )
                        })?;
                }
            }
            result
        } else {
            self.client
                .refresh(
                    local_account_id,
                    stored_credentials.provider_account_id(),
                    refresh_token,
                    now_ms,
                    stored_credentials.oauth_client_kind(),
                )
                .await
        };
        let current = self.credentials.require(local_account_id).map_err(|_| {
            TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                error_codes::CREDENTIAL_LOAD_FAILED,
            )
        })?;
        let same_siwc_grant = current.matches_siwc_grant(&stored_credentials);
        if !current.matches_snapshot(&stored_credentials) && !same_siwc_grant {
            // A newer authorization wins even if the old grant failed. Never
            // turn a concurrent login into a persisted reauthentication state.
            if current.oauth_client_kind() == stored_credentials.oauth_client_kind()
                && current.provider_account_id() == stored_credentials.provider_account_id()
                && current.provider_user_id() == stored_credentials.provider_user_id()
                && !current.is_agent_identity()
                && current.is_access_usable(now_ms, self.refresh_skew_ms)
            {
                return current.to_token_refresh().map_err(|_| superseded());
            }
            return Err(superseded());
        }
        if revision.is_some_and(|revision| revision.guard().is_none()) {
            return Err(superseded());
        }
        let refreshed = refresh_result?;
        let stored_credentials = if same_siwc_grant {
            current
        } else {
            stored_credentials
        };
        let updated = stored_credentials
            .apply_refresh(refreshed, now_ms)
            .map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_refresh_response",
                )
            })?;
        {
            // The on-disk secret write is synchronous. Hold the slot read
            // guard while saving so a removed/re-added slot cannot receive
            // the old provider result, even if it uses the same account id.
            let _revision_guard = revision
                .map(|revision| revision.guard().ok_or_else(superseded))
                .transpose()?;
            if updated.siwc_registration().is_some() {
                self.credentials
                    .checkpoint_siwc_rotation(&stored_credentials, Some(&updated))
                    .map_err(|_| {
                        TokenRefreshFailure::new(
                            TokenRefreshFailureKind::Transient,
                            "credential_persist_failed",
                        )
                    })?;
            }
            self.credentials.save(&updated).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    error_codes::CREDENTIAL_PERSIST_FAILED,
                )
            })?;
            if updated.siwc_registration().is_some() {
                self.credentials
                    .clear_siwc_rotation(local_account_id)
                    .map_err(|_| {
                        TokenRefreshFailure::new(
                            TokenRefreshFailureKind::Transient,
                            "credential_persist_failed",
                        )
                    })?;
            }
        }
        super::super::records::require_plan_permission(&updated).map_err(|_| {
            TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "siwc_plan_use_disabled")
        })?;
        updated.to_token_refresh().map_err(|_| {
            TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                "invalid_stored_credential",
            )
        })
    }
}

fn siwc_refresh_definitely_rejected(code: &str) -> bool {
    matches!(
        code,
        "siwc_refresh_too_early"
            | "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused"
            | "invalid_client"
            | "invalid_request"
            | "invalid_scope"
            | "unsupported_grant_type"
            | "access_denied"
    )
}

#[test]
fn siwc_uncertain_rotation_reply_must_keep_the_recovery_checkpoint() {
    for code in [
        "transport",
        "invalid_siwc_refresh_response",
        "token_refresh_failed",
        "server_error",
        "temporarily_unavailable",
    ] {
        assert!(!siwc_refresh_definitely_rejected(code));
    }
    for code in [
        "siwc_refresh_too_early",
        "invalid_grant",
        "refresh_token_reused",
        "invalid_client",
    ] {
        assert!(siwc_refresh_definitely_rejected(code));
    }
}

impl<B, C> TokenRefreshAdapter for StoredRefreshAdapter<B, C>
where
    B: SecretBackend + Send + Sync,
    C: CodexRefreshClient,
{
    fn refresh<'a>(
        &'a self,
        local_account_id: &'a str,
        _stale_refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<TokenRefresh, TokenRefreshFailure>> + Send + 'a>> {
        Box::pin(self.refresh_inner(local_account_id, now_ms, None))
    }

    fn refresh_fenced<'a>(
        &'a self,
        local_account_id: &'a str,
        _stale_refresh_token: &'a str,
        now_ms: u64,
        revision: &'a TokenDispatchRevision,
    ) -> Pin<Box<dyn Future<Output = Result<TokenRefresh, TokenRefreshFailure>> + Send + 'a>> {
        Box::pin(self.refresh_inner(local_account_id, now_ms, Some(revision)))
    }
}
