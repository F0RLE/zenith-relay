use super::import_orchestrator::credential_local_error;
use crate::local_pool::accounts::credentials::{CredentialStore, StoredCodexCredentials};
use crate::local_pool::accounts::exports::{
    finish_account_export, normalize_account_ids, AccountExportInput, AccountExportResult,
};
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::commands::current_time_ms;
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::DesktopState;
use std::collections::HashMap;
use tauri::{AppHandle, State};
use zenith_relay_core::accounts::{
    build_account_export, AccountExportCredential, AccountExportDocument, AccountExportFormat,
};
use zenith_relay_core::protocol::{RemoteAccountLocation, RevealedAccountIdentity};

type CommandResult<T> = std::result::Result<T, CommandError>;

#[tauri::command]
pub fn reveal_local_account_identity(
    account_id: String,
    state: State<'_, DesktopState>,
) -> CommandResult<RevealedAccountIdentity> {
    let account_id = normalize_account_ids(vec![account_id])?
        .pop()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::InvalidState, "account id is required"))?;
    {
        let store = state.store()?;
        if store.account(&account_id).is_none() {
            return Err(LocalPoolError::new(ErrorCode::NotFound, "account was not found").into());
        }
    }
    let credentials = CredentialStore::from_backend(NativeSecretBackend)
        .require(&account_id)
        .map_err(credential_local_error)?;
    let identity = revealable_account_identity(&credentials)
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "account identity is unavailable"))?
        .to_string();
    Ok(RevealedAccountIdentity {
        account_id,
        identity,
    })
}

pub(super) fn revealable_account_identity(credentials: &StoredCodexCredentials) -> Option<&str> {
    credentials
        .email()
        .or_else(|| credentials.provider_account_id())
        .or_else(|| credentials.provider_user_id())
}

pub(super) fn export_account_label(label: &str, credentials: &StoredCodexCredentials) -> String {
    if credentials.snapshot().identity.as_deref() == Some(label) {
        credentials.email().unwrap_or(label).to_string()
    } else {
        label.to_string()
    }
}

#[tauri::command]
pub fn export_local_accounts(
    input: AccountExportInput,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> CommandResult<AccountExportResult> {
    let account_ids = normalize_account_ids(input.account_ids)?;
    let document = build_local_account_export_document(
        &account_ids,
        input.format,
        input.description.as_deref(),
        &state,
    )?;
    finish_account_export(document, input.destination, &app)
}

pub(crate) fn build_local_account_export_document(
    account_ids: &[String],
    format: AccountExportFormat,
    description: Option<&str>,
    state: &DesktopState,
) -> LocalResult<AccountExportDocument> {
    let records = {
        let store = state.store()?;
        account_ids
            .iter()
            .map(|account_id| {
                store.account(account_id).cloned().ok_or_else(|| {
                    LocalPoolError::new(ErrorCode::NotFound, "account export item was not found")
                })
            })
            .collect::<LocalResult<Vec<_>>>()?
    };
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let accounts = records
        .iter()
        .map(|record| {
            let credentials = credential_store
                .require(&record.account.id)
                .map_err(credential_local_error)?;
            Ok(AccountExportCredential {
                label: export_account_label(&record.account.label, &credentials),
                email: credentials.email().map(str::to_string),
                access_token: credentials.access_token().to_string(),
                refresh_token: credentials.refresh_token().map(str::to_string),
                id_token: credentials.id_token().map(str::to_string),
                account_id: credentials.provider_account_id().map(str::to_string),
                user_id: credentials.provider_user_id().map(str::to_string),
                organization_id: credentials.organization_id().map(str::to_string),
                plan_type: record
                    .account
                    .subscription
                    .plan_type
                    .clone()
                    .or_else(|| credentials.plan_type().map(str::to_string)),
                expires_at_ms: credentials.expires_at_ms(),
                issued_at_ms: credentials.issued_at_ms(),
                subscription_active_until_ms: record.account.subscription.active_until_ms,
                created_at_ms: record.account.created_at_ms,
                priority: record.priority,
                enabled: record.account.enabled,
            })
        })
        .collect::<LocalResult<Vec<_>>>()?;
    let document = build_account_export(format, &accounts, current_time_ms(), description)
        .map_err(|error| LocalPoolError::new(ErrorCode::InvalidState, error.to_string()))?;
    Ok(document)
}

pub(crate) fn mark_local_accounts_moved(
    accounts: &mut [LocalAccountRecord],
    remote_locations: &HashMap<String, RemoteAccountLocation>,
) -> LocalResult<()> {
    let mut updated = 0;
    for account in accounts {
        if let Some(location) = remote_locations.get(&account.account.id) {
            account.account.enabled = false;
            account.account.in_pool = false;
            account.remote_location = Some(location.clone());
            updated += 1;
        }
    }
    if updated != remote_locations.len() {
        return Err(LocalPoolError::new(
            ErrorCode::NotFound,
            "an account selected for transfer was not found",
        ));
    }
    Ok(())
}
