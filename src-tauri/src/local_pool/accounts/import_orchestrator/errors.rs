use crate::local_pool::accounts::credentials::{CredentialError, CredentialErrorCode};
use crate::local_pool::accounts::import_session::{ImportSessionError, ImportSessionErrorCode};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use zenith_relay_core::error_codes;
use zenith_relay_core::normalize_error_code;
use zenith_relay_core::providers::chatgpt::ModelDiscoveryFailure;

pub(in crate::local_pool::accounts) type ItemResult<T> = std::result::Result<T, ImportItemError>;

pub(in crate::local_pool::accounts) use crate::local_pool::accounts::credentials::credential_local_error;

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItemError {
    pub code: String,
    pub message: String,
}

impl ImportItemError {
    pub(in crate::local_pool::accounts) fn new(code: &str, message: &str) -> Self {
        Self {
            code: normalize_error_code(code)
                .unwrap_or_else(|| error_codes::OPERATION_FAILED.to_string()),
            message: message.to_string(),
        }
    }

    pub(in crate::local_pool::accounts) fn recovery(message: &str) -> Self {
        Self::new(error_codes::RECOVERY_REQUIRED, message)
    }
}

pub(in crate::local_pool::accounts) fn credential_item_error(
    error: CredentialError,
) -> ImportItemError {
    let code = match error.code {
        CredentialErrorCode::InvalidIdentity => error_codes::INVALID_ACCOUNT_IDENTITY,
        CredentialErrorCode::InvalidSecret | CredentialErrorCode::InvalidVersion => {
            error_codes::INVALID_CREDENTIALS
        }
        CredentialErrorCode::SecretMissing => error_codes::CREDENTIALS_MISSING,
        CredentialErrorCode::SecretStoreUnavailable => error_codes::CREDENTIAL_STORE_UNAVAILABLE,
    };
    ImportItemError::new(code, &error.message)
}

pub(in crate::local_pool::accounts) fn import_item_command_error(
    error: ImportItemError,
) -> CommandError {
    let code = if error.code == error_codes::RECOVERY_REQUIRED {
        ErrorCode::RecoveryRequired
    } else {
        ErrorCode::InvalidState
    };
    LocalPoolError::new(code, error.message).into()
}

pub(in crate::local_pool::accounts) fn proxy_item_error(error: LocalPoolError) -> ImportItemError {
    ImportItemError::new(error_codes::PROXY_UNAVAILABLE, &error.message)
}

pub(in crate::local_pool::accounts) fn model_item_error(
    error: ModelDiscoveryFailure,
) -> ImportItemError {
    ImportItemError::new(model_failure_code(&error), &error.to_string())
}

pub(in crate::local_pool::accounts) fn model_failure_code(
    error: &ModelDiscoveryFailure,
) -> &'static str {
    error.code.management_code()
}

pub(in crate::local_pool::accounts) fn import_session_error(
    error: ImportSessionError,
) -> CommandError {
    let code = match error.code {
        ImportSessionErrorCode::SessionNotFound => ErrorCode::NotFound,
        ImportSessionErrorCode::SecretMissing => ErrorCode::RecoveryRequired,
        ImportSessionErrorCode::SecretStoreUnavailable => ErrorCode::SecretStoreUnavailable,
        ImportSessionErrorCode::CleanupIncomplete | ImportSessionErrorCode::RecoveryRequired => {
            ErrorCode::RecoveryRequired
        }
        ImportSessionErrorCode::SnapshotIo => ErrorCode::Io,
        _ => ErrorCode::InvalidState,
    };
    LocalPoolError::new(code, error.message).into()
}
