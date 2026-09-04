use crate::local_pool::accounts::credentials::{CredentialError, CredentialErrorCode};
use crate::local_pool::accounts::import_session::{ImportSessionError, ImportSessionErrorCode};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
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
            code: normalize_error_code(code).unwrap_or_else(|| "operation_failed".to_string()),
            message: message.to_string(),
        }
    }

    pub(in crate::local_pool::accounts) fn recovery(message: &str) -> Self {
        Self::new("recovery_required", message)
    }
}

pub(in crate::local_pool::accounts) fn credential_item_error(
    error: CredentialError,
) -> ImportItemError {
    let code = match error.code {
        CredentialErrorCode::InvalidIdentity => "invalid_account_identity",
        CredentialErrorCode::InvalidSecret | CredentialErrorCode::InvalidVersion => {
            "invalid_credentials"
        }
        CredentialErrorCode::SecretMissing => "credentials_missing",
        CredentialErrorCode::SecretStoreUnavailable => "credential_store_unavailable",
    };
    ImportItemError::new(code, &error.message)
}

pub(in crate::local_pool::accounts) fn import_item_command_error(
    error: ImportItemError,
) -> CommandError {
    let code = if error.code == "recovery_required" {
        ErrorCode::RecoveryRequired
    } else {
        ErrorCode::InvalidState
    };
    LocalPoolError::new(code, error.message).into()
}

pub(in crate::local_pool::accounts) fn proxy_item_error(error: LocalPoolError) -> ImportItemError {
    ImportItemError::new("proxy_unavailable", &error.message)
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
