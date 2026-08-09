use super::super::object_path;
use crate::local_pool::{
    accounts::export_ops::build_local_account_export_document,
    error::{CommandError, ErrorCode, LocalPoolError},
    remote::client::{RemoteClient, RemoteClientError},
    state::DesktopState,
};
use reqwest::Method;
use serde::Deserialize;
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};
use zenith_relay_core::accounts::{AccountAuthState, AccountExportFormat};
use zenith_relay_core::protocol::{valid_generated_id, AccountSummary, OperationalStatus};

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

fn validate_remote_transfer_preview(
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

fn validate_remote_transfer_confirmation(
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

fn remote_accounts_are_validated(accounts: &[AccountSummary], expected_ids: &[String]) -> bool {
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

fn invalid_remote_transfer(message: &str) -> CommandError {
    LocalPoolError::new(ErrorCode::InvalidState, message).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    #[test]
    fn transfer_confirmation_preserves_preview_order() {
        let preview = preview(false, true);
        let confirmation = RemoteBatchImportConfirmation {
            session_id: preview.session_id.clone(),
            results: vec![
                result(
                    "import_22222222222222222222222222222222",
                    "remote-two",
                    true,
                ),
                result(
                    "import_11111111111111111111111111111111",
                    "remote-one",
                    false,
                ),
            ],
        };

        assert_eq!(
            validate_remote_transfer_confirmation(&preview, confirmation)
                .unwrap()
                .account_ids,
            vec!["remote-one", "remote-two"]
        );
    }

    #[test]
    fn successful_transfer_tracks_only_new_accounts_for_rollback() {
        let preview = preview(true, false);
        let confirmation = RemoteBatchImportConfirmation {
            session_id: preview.session_id.clone(),
            results: vec![
                result(
                    "import_11111111111111111111111111111111",
                    "remote-existing",
                    false,
                ),
                result(
                    "import_22222222222222222222222222222222",
                    "remote-new",
                    true,
                ),
            ],
        };

        let confirmed = validate_remote_transfer_confirmation(&preview, confirmation).unwrap();

        assert_eq!(confirmed.account_ids, vec!["remote-existing", "remote-new"]);
        assert_eq!(confirmed.created_account_ids, vec!["remote-new"]);
    }

    #[test]
    fn rollback_uses_server_creation_status_instead_of_preview_state() {
        let preview = preview(true, false);
        let confirmation = RemoteBatchImportConfirmation {
            session_id: preview.session_id.clone(),
            results: vec![
                result(
                    "import_11111111111111111111111111111111",
                    "remote-existing",
                    true,
                ),
                result(
                    "import_22222222222222222222222222222222",
                    "remote-new",
                    false,
                ),
            ],
        };

        let confirmed = validate_remote_transfer_confirmation(&preview, confirmation).unwrap();

        assert_eq!(confirmed.created_account_ids, vec!["remote-existing"]);
    }

    #[test]
    fn rollback_delete_retries_only_transport_errors() {
        assert!(should_retry_remote_delete(&RemoteClientError::Transport));
        assert!(!should_retry_remote_delete(&RemoteClientError::HttpStatus(
            503
        )));
        assert!(!should_retry_remote_delete(
            &RemoteClientError::InvalidResponse
        ));
        assert_eq!(remote_delete_retry_delay(1), Duration::from_millis(100));
        assert_eq!(remote_delete_retry_delay(2), Duration::from_millis(200));
    }

    #[tokio::test]
    async fn rollback_delete_retries_after_a_transport_failure() {
        let (server, requests, task) = spawn_delete_server(vec![
            None,
            Some(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"),
        ])
        .await;
        let client = RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap();

        assert!(delete_remote_accounts(&client, &["remote-new".into()]).await);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn rollback_delete_accepts_an_already_missing_account() {
        let (server, requests, task) = spawn_delete_server(vec![Some(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )])
        .await;
        let client = RemoteClient::new(&server, "synthetic-management-token-value", false).unwrap();

        assert!(delete_remote_accounts(&client, &["remote-new".into()]).await);
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn transfer_preview_rejects_duplicate_item_ids() {
        let mut preview = preview(false, true);
        preview.preview.rows[1].item_id = preview.preview.rows[0].item_id.clone();

        assert!(validate_remote_transfer_preview(&preview, 2).is_err());
    }

    #[test]
    fn partial_transfer_reports_only_new_accounts_for_rollback() {
        let preview = preview(false, true);
        let confirmation = RemoteBatchImportConfirmation {
            session_id: preview.session_id.clone(),
            results: vec![
                result(
                    "import_11111111111111111111111111111111",
                    "remote-new",
                    true,
                ),
                RemoteBatchImportResult {
                    item_id: "import_22222222222222222222222222222222".into(),
                    status: "failed".into(),
                    account_id: None,
                    created: false,
                },
            ],
        };

        let error = validate_remote_transfer_confirmation(&preview, confirmation).unwrap_err();
        assert_eq!(error.created_account_ids, vec!["remote-new"]);
        assert!(!error.uncertain);
    }

    #[test]
    fn transfer_confirmation_rejects_duplicate_account_ids() {
        let preview = preview(false, false);
        let confirmation = RemoteBatchImportConfirmation {
            session_id: preview.session_id.clone(),
            results: vec![
                result(
                    "import_11111111111111111111111111111111",
                    "remote-same",
                    true,
                ),
                result(
                    "import_22222222222222222222222222222222",
                    "remote-same",
                    true,
                ),
            ],
        };

        assert!(validate_remote_transfer_confirmation(&preview, confirmation).is_err());
    }

    #[test]
    fn local_routing_waits_for_complete_remote_account_validation() {
        let mut account = validated_account("remote-one");
        assert!(remote_accounts_are_validated(
            &[account.clone()],
            &["remote-one".into()]
        ));

        account.last_error_code = Some("runtime_rebuild_failed".into());
        assert!(!remote_accounts_are_validated(
            &[account],
            &["remote-one".into()]
        ));
        assert!(!remote_accounts_are_validated(&[], &["remote-one".into()]));
    }

    fn preview(first_existing: bool, second_existing: bool) -> RemoteBatchImportSession {
        RemoteBatchImportSession {
            session_id: "batch_00000000000000000000000000000000".into(),
            prepared: true,
            preview: RemoteBatchImportPreview {
                rows: vec![
                    row("import_11111111111111111111111111111111", first_existing),
                    row("import_22222222222222222222222222222222", second_existing),
                ],
            },
        }
    }

    fn row(item_id: &str, existing: bool) -> RemoteBatchImportRow {
        RemoteBatchImportRow {
            item_id: item_id.into(),
            status: if existing { "existing" } else { "ready" }.into(),
            selectable: true,
        }
    }

    fn result(item_id: &str, account_id: &str, created: bool) -> RemoteBatchImportResult {
        RemoteBatchImportResult {
            item_id: item_id.into(),
            status: "succeeded".into(),
            account_id: Some(account_id.into()),
            created,
        }
    }

    fn validated_account(account_id: &str) -> AccountSummary {
        serde_json::from_value(serde_json::json!({
            "id": account_id,
            "label": "Synthetic account",
            "identityHint": "synthetic",
            "enabled": true,
            "inPool": true,
            "draining": false,
            "operationalStatus": "rotation",
            "authState": { "state": "active" },
            "health": "healthy",
            "models": ["gpt-test"],
            "allowedModels": [],
            "excludedModels": [],
            "priority": 0,
            "weight": 1,
            "apiEquivalent": { "microUsd": 0, "pricedTokens": 0, "unpricedTokens": 0 },
            "subscription": {
                "planType": "plus",
                "activeUntilMs": null,
                "status": "active",
                "updatedAtMs": 1
            },
            "quota": {
                "primary": null,
                "secondary": null,
                "supplemental": [],
                "limitReached": false,
                "resetCreditsAvailable": null,
                "updatedAtMs": 1,
                "error": null
            },
            "quotaRefreshStatus": "updated",
            "secretAvailable": true,
            "proxyMode": "direct",
            "proxyAvailable": true,
            "lastErrorCode": null
        }))
        .unwrap()
    }

    async fn spawn_delete_server(
        responses: Vec<Option<&'static [u8]>>,
    ) -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let observed = requests.clone();
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await.unwrap();
                observed.fetch_add(1, Ordering::SeqCst);
                if let Some(response) = response {
                    stream.write_all(response).await.unwrap();
                }
            }
        });
        (format!("http://{address}"), requests, task)
    }
}
