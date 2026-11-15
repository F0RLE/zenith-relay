use super::*;
use axum::http::{HeaderMap, header::AUTHORIZATION};
use axum::{Json, Router, routing::get};
use std::fs;
use tokio::net::TcpListener;
use url::Url;

#[tokio::test]
async fn siwc_import_round_trip_is_inactive_preserves_registration_and_never_probes_codex() {
    let root = crate::files::test_temp_root("relay-siwc-import");
    let mut state = DesktopState::open(root.clone()).unwrap();
    state.set_account_check_url_for_test(Url::parse("http://127.0.0.1:9/accounts/check").unwrap());
    let input = serde_json::json!({
        "oauth_client_kind": "siwc",
        "client_id": "oaiapp_synthetic_import",
        "access_token": "synthetic-siwc-access",
        "refresh_token": "synthetic-siwc-refresh",
        "siwc": {
            "clientId": "oaiapp_synthetic_import", "hostId": "urn:uuid:source-host",
            "issuer": "https://auth.openai.com", "subject": "synthetic-siwc-subject",
            "scopes": ["resource.invoke", "chatgpt.tokens.use.direct"],
            "requiresReauthentication": false
        }
    })
    .to_string();
    let sessions = ImportSessionStore::new(state.transient_root(), NativeSecretBackend);
    let session = sessions.start(&input, None, &[]).unwrap();
    let session_id = session.session_id.clone();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let (content, preview) = prepare_import_preview(&state, &credentials, session, true)
        .await
        .unwrap();
    assert_eq!(preview.rows.len(), 1);
    assert!(preview.rows[0].selectable);
    assert_eq!(preview.rows[0].quota_status, ImportQuotaStatus::Skipped);
    let prepared = sessions
        .prepare(&session_id, content.as_deref(), preview, &[])
        .unwrap();
    let selected = vec![prepared.items[0].item_id.clone()];
    let result = super::super::confirmation::confirm_local_account_import_inner(
        super::super::ConfirmAccountImportInput {
            session_id,
            selected_item_ids: selected,
            add_to_pool: true,
            discover_models: true,
            probe_quota: true,
            models: Vec::new(),
        },
        &state,
        None,
    )
    .await
    .unwrap();
    assert_eq!(result.results.len(), 1);
    assert!(matches!(
        result.results[0].status,
        super::super::ImportItemStatus::Succeeded
    ));
    let account = state.store().unwrap().accounts()[0].clone();
    assert!(!account.account.in_pool);
    assert!(account.discovered_models.is_none());
    let stored = credentials.require(&account.account.id).unwrap();
    let registration = stored.siwc_registration().unwrap();
    assert_eq!(registration.client_id, "oaiapp_synthetic_import");
    assert!(registration.requires_reauthentication);
    assert!(stored.authorization(current_time_ms()).is_err());
    assert!(crate::local_pool::accounts::records::require_plan_permission(&stored).is_err());
    let document = crate::local_pool::accounts::export_ops::build_local_account_export_document(
        std::slice::from_ref(&account.account.id),
        zenith_relay_core::accounts::AccountExportFormat::Zenith,
        None,
        &state,
    )
    .unwrap();
    let parsed = zenith_relay_core::accounts::parse_import(&document.content, None, &[]).unwrap();
    assert_eq!(
        parsed.items[0].secrets().siwc_registration(),
        Some(registration)
    );
    let (normalized, _) = normalize_import_input(StartAccountImportInput {
        content: None,
        documents: vec![document.content],
        source_file: None,
    })
    .unwrap();
    let duplicate = sessions.start(&normalized, None, &[]).unwrap();
    let result = super::super::confirmation::confirm_local_account_import_inner(
        super::super::ConfirmAccountImportInput {
            session_id: duplicate.session_id,
            selected_item_ids: vec![duplicate.items[0].item_id.clone()],
            add_to_pool: true,
            discover_models: true,
            probe_quota: true,
            models: Vec::new(),
        },
        &state,
        None,
    )
    .await
    .unwrap();
    assert!(matches!(
        result.results[0].status,
        super::super::ImportItemStatus::Failed
    ));
    assert!(
        credentials
            .require(&account.account.id)
            .unwrap()
            .matches_snapshot(&stored)
    );
    credentials.delete(&account.account.id).unwrap();
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn authenticated_free_plan_keeps_bps_and_native_import_previews_selectable() {
    let root = crate::files::test_temp_root("zenith-relay-import-free-bps");
    let mut state = DesktopState::open(root.clone()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|headers: HeaderMap| async move {
            let account_id = if headers[AUTHORIZATION] == "Bearer synthetic-bps-paid" {
                "synthetic-free-known"
            } else {
                "synthetic-free"
            };
            Json(serde_json::json!({
                "accounts": [{
                    "account": {"id": account_id, "plan_type": "free"}
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
                {"oauth_client_kind":"excel_bps","account_id":"synthetic-free-known","access_token":"synthetic-bps-paid","plan_type":"plus"},
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
    for row in &preview.rows {
        assert!(row.selectable);
        assert!(row.error.is_none());
        assert_eq!(row.plan.as_deref(), Some("free"));
    }
    let prepared = sessions
        .prepare(&session_id, content.as_deref(), preview, &[])
        .unwrap();
    assert_eq!(prepared.items.len(), 3);
    assert!(state.store().unwrap().accounts().is_empty());
    sessions.cancel(&session_id).unwrap();
    server.abort();
    drop(state);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn preparation_persists_only_credentials_for_selectable_rows() {
    let root = crate::files::test_temp_root("zenith-relay-import-preview");
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
    let root = crate::files::test_temp_root("zenith-relay-import-rejected-token");
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
    let root = crate::files::test_temp_root("zenith-relay-import-duplicate-preview");
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

#[tokio::test]
async fn prepared_import_keeps_both_expiries_in_the_reparsed_credential() {
    let (endpoint, server) = spawn_account_check_server().await;
    let mut parsed = zenith_relay_core::accounts::parse_import(
        r#"{"access_token":"synthetic-access-ok","expires_at":"2026-10-19T14:17:45Z"}"#,
        None,
        &[],
    )
    .unwrap();
    let original = parsed_item_json(
        &parsed.items[0],
        zenith_relay_core::accounts::ImportAuthMode::ImportedToken,
    );
    let mut material = build_import_credential_material(
        parsed.items.remove(0),
        1,
        Some("plus"),
        Some(1_792_419_465_000),
        None,
        2,
        &endpoint,
    )
    .await
    .unwrap();
    for expiry in [Some(1_792_419_465_000), None] {
        material.expires_at_ms = expiry;
        let prepared = parsed_item_json_with_material(original.clone(), &material);
        let reparsed =
            zenith_relay_core::accounts::parse_import(&prepared.to_string(), None, &[]).unwrap();
        assert_eq!(reparsed.items[0].expires_at_ms, expiry);
        assert_eq!(
            reparsed.preview.rows[0]
                .subscription_expires_at
                .as_deref()
                .and_then(super::super::parse_subscription_timestamp_ms),
            material.subscription_active_until_ms,
        );
    }
    server.abort();
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
