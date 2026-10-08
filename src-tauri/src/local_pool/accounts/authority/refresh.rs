use super::super::{
    credentials::{CredentialRefresh, CredentialStore},
    import_session::SecretBackend,
    oauth::CodexOAuthClient,
};
use super::lock::{lock_refresh_failure, ProcessAccountLocks, ProcessLockConfig, ProcessLockError};
use std::{future::Future, path::PathBuf, pin::Pin, sync::Arc};
use zenith_relay_core::accounts::{
    TokenDispatchRevision, TokenRefresh, TokenRefreshAdapter, TokenRefreshFailure,
    TokenRefreshFailureKind,
};
use zenith_relay_core::error_codes;

pub trait CodexRefreshClient: Send + Sync {
    fn refresh<'a>(
        &'a self,
        local_account_id: &'a str,
        provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>;
}

impl CodexRefreshClient for CodexOAuthClient {
    fn refresh<'a>(
        &'a self,
        _local_account_id: &'a str,
        _provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            let tokens = self.exchange_refresh_token(refresh_token, now_ms).await?;
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
        let stored_credentials = self.credentials.require(local_account_id).map_err(|_| {
            TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                error_codes::CREDENTIAL_LOAD_FAILED,
            )
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
        let refreshed = self
            .client
            .refresh(
                local_account_id,
                stored_credentials.provider_account_id(),
                refresh_token,
                now_ms,
            )
            .await?;
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
            self.credentials.save(&updated).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    error_codes::CREDENTIAL_PERSIST_FAILED,
                )
            })?;
        }
        updated.to_token_refresh().map_err(|_| {
            TokenRefreshFailure::new(
                TokenRefreshFailureKind::Transient,
                "invalid_stored_credential",
            )
        })
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
