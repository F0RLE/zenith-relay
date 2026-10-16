use super::authority::{ProcessAccountLocks, ProcessLockConfig};
use super::credentials::{totp_code, CredentialStore, StoredCodexCredentials};
use super::exports::normalize_one_account_id;
use super::import_orchestrator::credential_local_error;
use super::NativeSecretBackend;
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use crate::local_pool::state::DesktopState;
use serde::{Deserialize, Serialize};
use std::fmt;
use tauri::State;
use zenith_relay_core::accounts::normalize_login_totp_secret;

type CommandResult<T> = std::result::Result<T, CommandError>;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountLoginUpdate {
    pub account_id: String,
    pub email: String,
    pub phone: String,
    pub password: String,
    pub totp_secret: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountLoginDetails {
    pub account_id: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub password: Option<String>,
    pub totp_secret: Option<String>,
    pub totp_code: Option<String>,
    pub totp_expires_at_ms: Option<u64>,
}

impl fmt::Debug for AccountLoginDetails {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AccountLoginDetails")
            .field("account_id", &self.account_id)
            .field("email", &self.email.as_ref().map(|_| "[redacted]"))
            .field("phone", &self.phone.as_ref().map(|_| "[redacted]"))
            .field("password", &self.password.as_ref().map(|_| "[redacted]"))
            .field(
                "totp_secret",
                &self.totp_secret.as_ref().map(|_| "[redacted]"),
            )
            .field("totp_code", &self.totp_code.as_ref().map(|_| "[redacted]"))
            .field("totp_expires_at_ms", &self.totp_expires_at_ms)
            .finish()
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountTotpPreview {
    pub code: Option<String>,
    pub expires_at_ms: Option<u64>,
}

#[tauri::command]
pub fn reveal_local_account_login(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<AccountLoginDetails> {
    let account_id = normalize_one_account_id(account_id)?;
    let credentials = load_account_credentials(&account_id, &state)?;
    Ok(login_details(&account_id, &credentials, current_time_ms()))
}

#[tauri::command]
pub async fn update_local_account_login(
    input: AccountLoginUpdate,
    state: State<'_, DesktopState>,
) -> CommandResult<AccountLoginDetails> {
    let account_id = normalize_one_account_id(input.account_id)?;
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .map_err(|_| {
                LocalPoolError::new(
                    ErrorCode::InvalidState,
                    "account credential lock is unavailable",
                )
            })?;
    let _lock = locks.acquire(&account_id).await.map_err(|_| {
        LocalPoolError::new(
            ErrorCode::Conflict,
            "account credentials are being refreshed",
        )
    })?;
    let credentials = load_account_credentials(&account_id, &state)?;
    let updated = credentials
        .replace_login_notes(input.email, input.phone, input.password, input.totp_secret)
        .map_err(|_| {
            LocalPoolError::new(ErrorCode::InvalidState, "account login note is invalid")
        })?;
    CredentialStore::from_backend(NativeSecretBackend)
        .save(&updated)
        .map_err(credential_local_error)?;
    Ok(login_details(&account_id, &updated, current_time_ms()))
}

#[tauri::command]
pub fn preview_totp_code(secret: String) -> AccountTotpPreview {
    let Some(secret) = normalize_login_totp_secret(&secret) else {
        return AccountTotpPreview {
            code: None,
            expires_at_ms: None,
        };
    };
    match totp_code(&secret, current_time_ms()) {
        Some(code) => AccountTotpPreview {
            code: Some(code.code),
            expires_at_ms: Some(code.expires_at_ms),
        },
        None => AccountTotpPreview {
            code: None,
            expires_at_ms: None,
        },
    }
}

fn load_account_credentials(
    account_id: &str,
    state: &DesktopState,
) -> Result<StoredCodexCredentials, LocalPoolError> {
    if state.store()?.account(account_id).is_none() {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "account was not found",
        ));
    }
    CredentialStore::from_backend(NativeSecretBackend)
        .require(account_id)
        .map_err(credential_local_error)
}

fn login_details(
    account_id: &str,
    credentials: &StoredCodexCredentials,
    now_ms: u64,
) -> AccountLoginDetails {
    let totp = credentials
        .totp_secret()
        .and_then(|secret| totp_code(secret, now_ms));
    AccountLoginDetails {
        account_id: account_id.to_string(),
        email: credentials.email().map(str::to_string),
        phone: credentials.phone().map(str::to_string),
        password: credentials.password().map(str::to_string),
        totp_secret: credentials.totp_secret().map(str::to_string),
        totp_code: totp.as_ref().map(|code| code.code.clone()),
        totp_expires_at_ms: totp.map(|code| code.expires_at_ms),
    }
}
