use crate::local_pool::error::{ErrorCode, LocalPoolError};
pub use zenith_relay_core::providers::chatgpt::connector::credentials::{
    CredentialError, CredentialErrorCode, CredentialRefresh, CredentialStore,
    StoredCodexCredentials, bearer_authorization, credential_secret_ref, totp_code,
};

pub(in crate::local_pool) fn credential_local_error(error: CredentialError) -> LocalPoolError {
    let code = match error.code {
        CredentialErrorCode::SecretMissing => ErrorCode::NotFound,
        CredentialErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        CredentialErrorCode::RotationOutcomeUnknown
        | CredentialErrorCode::RotationCheckpointInvalid => ErrorCode::RecoveryRequired,
        _ => ErrorCode::InvalidState,
    };
    LocalPoolError::new(code, error.message)
}

pub(in crate::local_pool) fn credential_invalid_state_error(
    error: CredentialError,
) -> LocalPoolError {
    LocalPoolError::new(ErrorCode::InvalidState, error.message)
}
