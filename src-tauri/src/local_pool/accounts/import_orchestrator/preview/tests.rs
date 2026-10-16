use super::*;
use axum::http::{header::AUTHORIZATION, HeaderMap};
use axum::{routing::get, Json, Router};
use std::fs;
use tokio::net::TcpListener;
use url::Url;
use uuid::Uuid;

#[tokio::test]
async fn authenticated_free_plan_rejects_bps_preview_but_keeps_native_import() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-free-bps-{}",
        Uuid::new_v4().simple()
    ));
    let mut state = DesktopState::open(root.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|| async {
            Json(serde_json::json!({
                "accounts": [{
                    "account": {"id": "synthetic-free", "plan_type": "free"}
                }]
            }))
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    state.set_account_check_url_for_test(
        Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
    );
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .start(
            r#"[
                {"oauth_client_kind":"excel_bps","access_token":"synthetic-bps-unknown"},
                {"oauth_client_kind":"excel_bps","account_id":"synthetic-free","access_token":"synthetic-bps-paid","plan_type":"plus"},
                {"oauth_client_kind":"codex","account_id":"synthetic-free","access_token":"synthetic-native","plan_type":"plus"}
            ]"#,
            None,
            &[],
        )
        .unwrap();
    let session_id = session.session_id.clone();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let (content, preview) = prepare_import_preview(&state, &credentials, session, false)
        .await
        .unwrap();
    assert_eq!(preview.rows.len(), 3);
    for row in &preview.rows[..2] {
        assert!(!row.selectable);
        assert_eq!(
            row.error.as_ref().map(|error| error.code),
            Some(ImportIssueCode::BasisPointsFreeAccountUnsupported)
        );
    }
    assert!(preview.rows[2].selectable);
    assert_eq!(preview.rows[2].plan.as_deref(), Some("free"));
    let prepared = sessions
        .prepare(&session_id, content.as_deref(), preview, &[])
        .unwrap();
    assert_eq!(prepared.items.len(), 1);
    assert!(state.store().unwrap().accounts().is_empty());
    sessions.cancel(&session_id).unwrap();
    server.abort();
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn preparation_persists_only_credentials_for_selectable_rows() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-preview-{}",
        Uuid::new_v4().simple()
    ));
    let mut state = DesktopState::open(root.clone()).unwrap();
    let (endpoint, server) = spawn_account_check_server().await;
    state.set_account_check_url_for_test(endpoint);

    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .start(
            r#"[
                {"auth_mode":"oauth","account_id":"synthetic-provider-ok","access_token":"synthetic-access-ok","refresh_token":"synthetic-refresh-ok"},
                {"auth_mode":"oauth","access_token":"synthetic-access-rejected","refresh_token":"synthetic-refresh-rejected"}
            ]"#,
            None,
            &[],
        )
        .unwrap();
    let session_id = session.session_id.clone();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);

    let (prepared_content, preview) = prepare_import_preview(&state, &credentials, session, false)
        .await
        .unwrap();
    let prepared_content = prepared_content.expect("filtered credentials must be persisted");
    let prepared_values =
        zenith_relay_core::accounts::parse_import(&prepared_content, None, &[]).unwrap();

    assert_eq!(prepared_values.items.len(), 1);
    assert_eq!(preview.rows.len(), 2);
    assert_eq!(preview.rows.iter().filter(|row| row.selectable).count(), 1);
    assert!(preview.rows[0].selectable);
    assert!(!preview.rows[1].selectable);

    let prepared = sessions
        .prepare(&session_id, Some(&prepared_content), preview.clone(), &[])
        .unwrap();
    assert_eq!(prepared.items.len(), 1);
    assert_eq!(prepared.preview, preview);

    sessions.cancel(&session_id).unwrap();
    server.abort();
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn preparation_keeps_a_rejected_token_when_the_account_id_is_known() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-rejected-token-{}",
        Uuid::new_v4().simple()
    ));
    let mut state = DesktopState::open(root.clone()).unwrap();
    let (endpoint, server) = spawn_rejected_account_check_server().await;
    state.set_account_check_url_for_test(endpoint);

    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .start(
            r#"{"auth_mode":"oauth","account_id":"synthetic-provider-rejected","access_token":"synthetic-access-rejected","refresh_token":"synthetic-refresh-rejected"}"#,
            None,
            &[],
        )
        .unwrap();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);

    let (prepared_content, preview) = prepare_import_preview(&state, &credentials, session, false)
        .await
        .unwrap();

    assert!(prepared_content.is_none());
    assert_eq!(preview.rows.len(), 1);
    assert!(preview.rows[0].selectable);
    assert!(preview.rows[0].error.is_none());
    assert_eq!(preview.rows[0].identity, "Account ****cted");

    server.abort();
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn preparation_filters_duplicates_found_by_authenticated_identity() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-duplicate-preview-{}",
        Uuid::new_v4().simple()
    ));
    let mut state = DesktopState::open(root.clone()).unwrap();
    let (endpoint, server) = spawn_duplicate_account_check_server().await;
    state.set_account_check_url_for_test(endpoint);

    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions
        .start(
            r#"[
                {"auth_mode":"oauth","access_token":"synthetic-access-one"},
                {"auth_mode":"oauth","access_token":"synthetic-access-two"}
            ]"#,
            None,
            &[],
        )
        .unwrap();
    let session_id = session.session_id.clone();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);

    let (prepared_content, preview) = prepare_import_preview(&state, &credentials, session, false)
        .await
        .unwrap();
    let prepared_content = prepared_content.expect("duplicate credentials must be filtered");
    let prepared_values =
        zenith_relay_core::accounts::parse_import(&prepared_content, None, &[]).unwrap();

    assert_eq!(prepared_values.items.len(), 1);
    assert_eq!(preview.rows.len(), 2);
    assert_eq!(preview.rows.iter().filter(|row| row.selectable).count(), 1);
    assert_eq!(
        preview.rows[1].error.as_ref().map(|error| error.code),
        Some(ImportIssueCode::DuplicateItem)
    );

    let prepared = sessions
        .prepare(&session_id, Some(&prepared_content), preview, &[])
        .unwrap();
    assert_eq!(prepared.items.len(), 1);

    sessions.cancel(&session_id).unwrap();
    server.abort();
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

async fn spawn_account_check_server() -> (Url, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|headers: HeaderMap| async move {
            let valid = headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value == "Bearer synthetic-access-ok");
            let payload = if valid {
                serde_json::json!({
                    "accounts": [{"account": {"id": "synthetic-provider-ok"}}]
                })
            } else {
                serde_json::json!({"accounts": []})
            };
            Json(payload)
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
        server,
    )
}

async fn spawn_rejected_account_check_server() -> (Url, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|| async { axum::http::StatusCode::UNAUTHORIZED }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
        server,
    )
}

async fn spawn_duplicate_account_check_server() -> (Url, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|headers: HeaderMap| async move {
            let valid = headers
                .get(AUTHORIZATION)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| {
                    matches!(
                        value,
                        "Bearer synthetic-access-one" | "Bearer synthetic-access-two"
                    )
                });
            let payload = if valid {
                serde_json::json!({
                    "accounts": [{"account": {"id": "synthetic-shared-account"}}]
                })
            } else {
                serde_json::json!({"accounts": []})
            };
            Json(payload)
        }),
    );
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (
        Url::parse(&format!("http://{address}/accounts/check")).unwrap(),
        server,
    )
}
