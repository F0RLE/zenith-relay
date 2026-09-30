use super::super::object_path;
use crate::local_pool::{
    accounts::export_ops::build_local_account_export_document,
    error::ErrorCode,
    remote::client::{RemoteClient, RemoteClientError},
    state::DesktopState,
};
use reqwest::Method;
use serde::Deserialize;
use std::time::Duration;
use zenith_relay_core::accounts::AccountExportFormat;

mod validate;

use validate::{
    remote_accounts_are_validated, validate_remote_transfer_confirmation,
    validate_remote_transfer_preview,
};

const REMOTE_DELETE_MAX_ATTEMPTS: u32 = 3;
const REMOTE_DELETE_RETRY_DELAY_MS: u64 = 100;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportSession {
    session_id: String,
    prepared: bool,
    preview: RemoteBatchImportPreview,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportPreview {
    rows: Vec<RemoteBatchImportRow>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportRow {
    item_id: String,
    status: String,
    selectable: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportConfirmation {
    session_id: String,
    results: Vec<RemoteBatchImportResult>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteBatchImportResult {
    item_id: String,
    status: String,
    #[serde(default)]
    account_id: Option<String>,
    created: bool,
}

#[derive(Debug)]
struct RemoteTransferConfirmationError {
    message: &'static str,
    created_account_ids: Vec<String>,
    uncertain: bool,
}

#[derive(Debug)]
struct RemoteTransferConfirmation {
    account_ids: Vec<String>,
    created_account_ids: Vec<String>,
}

pub(super) struct RemoteTransferBatch {
    pub(super) account_ids: Vec<String>,
    pub(super) created_account_ids: Vec<String>,
}

pub(super) struct RemoteTransferBatchError {
    pub(super) code: ErrorCode,
    pub(super) message: String,
    pub(super) created_account_ids: Vec<String>,
}

pub(super) async fn transfer_local_account_batch(
    state: &DesktopState,
    client: &RemoteClient,
    account_ids: &[String],
) -> Result<RemoteTransferBatch, RemoteTransferBatchError> {
    let document =
        build_local_account_export_document(account_ids, AccountExportFormat::Zenith, None, state)
            .map_err(|error| RemoteTransferBatchError {
                code: error.code,
                message: error.message,
                created_account_ids: Vec::new(),
            })?;
    let preview_value = client
        .mutate(
            Method::POST,
            "/accounts/import/batch/preview",
            Some(&serde_json::json!({ "content": document.content })),
        )
        .await
        .map_err(|error| RemoteTransferBatchError {
            code: ErrorCode::GatewayUnavailable,
            message: error.to_string(),
            created_account_ids: Vec::new(),
        })?;
    let preview: RemoteBatchImportSession =
        serde_json::from_value(preview_value).map_err(|_| RemoteTransferBatchError {
            code: ErrorCode::InvalidState,
            message: "remote import preview is invalid".into(),
            created_account_ids: Vec::new(),
        })?;
    validate_remote_transfer_preview(&preview, account_ids.len()).map_err(|error| {
        RemoteTransferBatchError {
            code: error.code,
            message: error.message,
            created_account_ids: Vec::new(),
        }
    })?;
    let selected_item_ids = preview
        .preview
        .rows
        .iter()
        .map(|row| row.item_id.clone())
        .collect::<Vec<_>>();
    let confirmation_value = client
        .mutate(
            Method::POST,
            "/accounts/import/batch/confirm",
            Some(&serde_json::json!({
                "sessionId": &preview.session_id,
                "selectedItemIds": selected_item_ids,
                "addToPool": true,
                "probeMetadata": true,
            })),
        )
        .await
        .map_err(|_| RemoteTransferBatchError {
            code: ErrorCode::RecoveryRequired,
            message: "remote import confirmation could not be verified".into(),
            created_account_ids: Vec::new(),
        })?;
    let confirmation: RemoteBatchImportConfirmation = serde_json::from_value(confirmation_value)
        .map_err(|_| RemoteTransferBatchError {
            code: ErrorCode::RecoveryRequired,
            message: "remote import confirmation is invalid".into(),
            created_account_ids: Vec::new(),
        })?;
    let confirmed =
        validate_remote_transfer_confirmation(&preview, confirmation).map_err(|error| {
            RemoteTransferBatchError {
                code: if error.uncertain {
                    ErrorCode::RecoveryRequired
                } else {
                    ErrorCode::GatewayUnavailable
                },
                message: error.message.into(),
                created_account_ids: error.created_account_ids,
            }
        })?;
    let remote_account_ids = confirmed.account_ids;
    let created_account_ids = confirmed.created_account_ids;
    let snapshot = client
        .state()
        .await
        .map_err(|error| RemoteTransferBatchError {
            code: ErrorCode::GatewayUnavailable,
            message: error.to_string(),
            created_account_ids: created_account_ids.clone(),
        })?;
    if !remote_accounts_are_validated(&snapshot.accounts, &remote_account_ids) {
        return Err(RemoteTransferBatchError {
            code: ErrorCode::GatewayUnavailable,
            message: "remote account validation did not complete successfully".into(),
            created_account_ids,
        });
    }
    Ok(RemoteTransferBatch {
        account_ids: remote_account_ids,
        created_account_ids,
    })
}

pub(super) async fn delete_remote_accounts(client: &RemoteClient, account_ids: &[String]) -> bool {
    let mut complete = true;
    for account_id in account_ids {
        let Ok(path) = object_path("accounts", account_id) else {
            complete = false;
            continue;
        };
        if !delete_remote_account(client, &path).await {
            complete = false;
        }
    }
    complete
}

async fn delete_remote_account(client: &RemoteClient, path: &str) -> bool {
    for attempt in 1..=REMOTE_DELETE_MAX_ATTEMPTS {
        match client.mutate(Method::DELETE, path, None).await {
            Ok(_) | Err(RemoteClientError::HttpStatus(404)) => return true,
            Err(error)
                if should_retry_remote_delete(&error) && attempt < REMOTE_DELETE_MAX_ATTEMPTS =>
            {
                tokio::time::sleep(remote_delete_retry_delay(attempt)).await;
            }
            Err(_) => return false,
        }
    }
    false
}

fn should_retry_remote_delete(error: &RemoteClientError) -> bool {
    matches!(error, RemoteClientError::Transport)
}

fn remote_delete_retry_delay(attempt: u32) -> Duration {
    Duration::from_millis(REMOTE_DELETE_RETRY_DELAY_MS * (1_u64 << (attempt - 1)))
}
#[cfg(test)]
mod tests;
