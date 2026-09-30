use super::super::super::object_path;
use super::{
    RemoteBatchImportConfirmation, RemoteBatchImportSession, RemoteTransferConfirmation,
    RemoteTransferConfirmationError,
};
use crate::local_pool::error::{CommandError, ErrorCode, LocalPoolError};
use std::collections::{HashMap, HashSet};
use zenith_relay_core::accounts::AccountAuthState;
use zenith_relay_core::protocol::{valid_generated_id, AccountSummary, OperationalStatus};

pub(super) fn validate_remote_transfer_preview(
    preview: &RemoteBatchImportSession,
    expected_accounts: usize,
) -> Result<(), CommandError> {
    if !preview.prepared
        || !valid_generated_id(&preview.session_id, "batch_")
        || preview.preview.rows.len() != expected_accounts
    {
        return Err(invalid_remote_transfer(
            "remote import preview is incomplete",
        ));
    }
    let mut seen = HashSet::new();
    if preview.preview.rows.iter().any(|row| {
        !row.selectable
            || !matches!(row.status.as_str(), "ready" | "existing")
            || !valid_generated_id(&row.item_id, "import_")
            || !seen.insert(row.item_id.as_str())
    }) {
        return Err(invalid_remote_transfer(
            "remote server rejected one or more selected accounts",
        ));
    }
    Ok(())
}

pub(super) fn validate_remote_transfer_confirmation(
    preview: &RemoteBatchImportSession,
    confirmation: RemoteBatchImportConfirmation,
) -> Result<RemoteTransferConfirmation, RemoteTransferConfirmationError> {
    let mut complete = confirmation.session_id == preview.session_id
        && confirmation.results.len() == preview.preview.rows.len();
    let mut uncertain = !complete;
    let mut results = HashMap::with_capacity(confirmation.results.len());
    for result in confirmation.results {
        if results.insert(result.item_id.clone(), result).is_some() {
            complete = false;
            uncertain = true;
        }
    }
    let mut account_ids = Vec::with_capacity(preview.preview.rows.len());
    let mut created_account_ids = Vec::new();
    let mut seen_account_ids = HashSet::with_capacity(preview.preview.rows.len());
    for row in &preview.preview.rows {
        let Some(result) = results.remove(&row.item_id) else {
            complete = false;
            uncertain = true;
            continue;
        };
        if result.status != "succeeded" {
            complete = false;
            continue;
        }
        let Some(account_id) = result.account_id else {
            complete = false;
            uncertain = true;
            continue;
        };
        if object_path("accounts", &account_id).is_err() {
            complete = false;
            uncertain = true;
            continue;
        }
        if !seen_account_ids.insert(account_id.clone()) {
            complete = false;
            uncertain = true;
            continue;
        }
        if result.created {
            created_account_ids.push(account_id.clone());
        }
        account_ids.push(account_id);
    }
    if !results.is_empty() {
        complete = false;
        uncertain = true;
    }
    if !complete || account_ids.len() != preview.preview.rows.len() {
        return Err(RemoteTransferConfirmationError {
            message: "remote server did not confirm every selected account",
            created_account_ids,
            uncertain,
        });
    }
    Ok(RemoteTransferConfirmation {
        account_ids,
        created_account_ids,
    })
}

pub(super) fn remote_accounts_are_validated(
    accounts: &[AccountSummary],
    expected_ids: &[String],
) -> bool {
    expected_ids.iter().all(|account_id| {
        accounts
            .iter()
            .find(|account| account.id == *account_id)
            .is_some_and(|account| {
                account.enabled
                    && account.in_pool
                    && !account.draining
                    && account.secret_available
                    && account.proxy_available
                    && account.auth_state == AccountAuthState::Active
                    && !account.models.is_empty()
                    && account.quota.updated_at_ms.is_some()
                    && account.quota.error.is_none()
                    && !matches!(
                        account.last_error_code.as_deref(),
                        Some("metadata_refresh_failed" | "runtime_rebuild_failed")
                    )
                    && matches!(
                        account.operational_status,
                        OperationalStatus::Rotation | OperationalStatus::QuotaWait
                    )
            })
    })
}

fn invalid_remote_transfer(message: &str) -> CommandError {
    LocalPoolError::new(ErrorCode::InvalidState, message).into()
}
