use super::super::{existing_identity_index, parse_subscription_timestamp_ms, ImportRowContext};
use super::{import_account_item, AccountImportOptions};
use crate::local_pool::accounts::credentials::CredentialStore;
use crate::local_pool::accounts::NativeSecretBackend;
use crate::local_pool::error::{ErrorCode, LocalPoolError, Result as LocalResult};
use crate::local_pool::models::LocalAccountRecord;
use crate::local_pool::state::DesktopState;
use zenith_relay_core::accounts::parse_import;
use zenith_relay_core::error_codes;

pub(crate) async fn stage_returned_remote_account(
    state: &DesktopState,
    local_account_id: &str,
    content: &str,
) -> LocalResult<LocalAccountRecord> {
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let existing = existing_identity_index(state, &credentials)?;
    let mut parsed = parse_import(content, None, &existing.keys().cloned().collect::<Vec<_>>())
        .map_err(LocalPoolError::invalid_state)?;
    if parsed.items.len() != 1 || parsed.preview.rows.len() != 1 {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote account export must contain exactly one account",
        ));
    }
    let row = parsed.preview.rows.remove(0);
    if !row.selectable {
        return Err(LocalPoolError::new(
            ErrorCode::InvalidState,
            "remote account export is not usable",
        ));
    }
    let existing_record = state
        .store()?
        .account(local_account_id)
        .cloned()
        .ok_or_else(|| LocalPoolError::new(ErrorCode::NotFound, "local account not found"))?;
    let row_context = ImportRowContext {
        label: row.label,
        auth_mode: row.auth_mode,
        selectable: row.selectable,
        plan: row.plan,
        subscription_active_until_ms: row
            .subscription_expires_at
            .as_deref()
            .and_then(parse_subscription_timestamp_ms),
    };
    let configured_models = existing_record.models.clone();
    let item = parsed.items.remove(0);
    let (account, _) = import_account_item(
        state,
        &credentials,
        item,
        &row_context,
        AccountImportOptions {
            add_to_pool: false,
            discover_models: true,
            probe_quota: true,
            configured_models: &configured_models,
        },
        state.account_check_url(),
    )
    .await
    .map_err(|error| {
        LocalPoolError::new(
            if error.code == error_codes::RECOVERY_REQUIRED {
                ErrorCode::RecoveryRequired
            } else {
                ErrorCode::InvalidState
            },
            error.message,
        )
    })?;
    if account.account.id != local_account_id
        || account.remote_location != existing_record.remote_location
        || account.account.enabled
        || account.account.in_pool
    {
        return Err(LocalPoolError::new(
            ErrorCode::RecoveryRequired,
            "returned credentials did not stage on the expected inactive local account",
        ));
    }
    Ok(account)
}
