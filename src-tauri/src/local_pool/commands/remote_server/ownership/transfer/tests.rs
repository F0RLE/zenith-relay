use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use zenith_relay_core::protocol::AccountSummary;

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
        "basisPointsAvailable": false,
        "basisPointsEnabled": false,
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
