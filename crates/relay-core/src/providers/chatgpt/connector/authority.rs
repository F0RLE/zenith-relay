//! Shared credential renewal and durable commit; the host owns locking and telemetry.
mod persistence;
mod refresh;

use crate::accounts::{TokenRefreshFailure, TokenRefreshFailureKind};
use futures_util::future::BoxFuture;

pub use persistence::{AccountMetadataSink, CredentialPersistence, MetadataSinkError};
pub use refresh::{CodexRefreshClient, StoredRefreshAdapter};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialLockError {
    InvalidConfiguration,
    InvalidIdentity,
    Io,
    Timeout,
    UnsafePath,
}

/// The returned guard must exclude login replacement, deletion and refresh for
/// this identity until dropped, including across asynchronous metadata writes.
pub trait CredentialLocks: Send + Sync {
    fn acquire<'a>(
        &'a self,
        account_id: &'a str,
    ) -> BoxFuture<'a, Result<Box<dyn Send>, CredentialLockError>>;

    /// Hosts decide whether to record diagnostics and redact identifiers.
    fn renewal_failed(&self, _account_id: &str, _failure: &TokenRefreshFailure) {}
}

fn lock_refresh_failure(error: CredentialLockError) -> TokenRefreshFailure {
    use crate::error_codes;
    let code = match error {
        CredentialLockError::Timeout => error_codes::REFRESH_LOCK_TIMEOUT,
        CredentialLockError::InvalidIdentity => error_codes::INVALID_ACCOUNT_ID,
        CredentialLockError::InvalidConfiguration => error_codes::REFRESH_LOCK_CONFIGURATION,
        CredentialLockError::Io | CredentialLockError::UnsafePath => {
            error_codes::REFRESH_LOCK_UNAVAILABLE
        }
    };
    TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, code)
}
