use super::super::super::{
    ConfirmAccountImportInput, ImportItemStatus, confirmation::confirm_local_account_import_inner,
};
use super::*;
use crate::local_pool::accounts::{export_ops::build_local_account_export_document, proxy};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use zenith_relay_core::accounts::AccountExportFormat;

async fn counting_endpoint() -> (Url, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    let hits = Arc::new(AtomicUsize::default());
    let count = hits.clone();
    let router = Router::new().fallback(move |headers: HeaderMap| {
        count.fetch_add(1, Ordering::SeqCst);
        async move {
            let id = headers[AUTHORIZATION]
                .to_str()
                .unwrap()
                .strip_prefix("Bearer ")
                .unwrap();
            Json(serde_json::json!({"accounts":[{"account":{"id":id, "plan_type":"plus"}}]}))
        }
    });
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (url, hits, server)
}

async fn preview(state: &DesktopState, ids: &[&str], url: &Url) -> ImportSessionResponse {
    let accounts: Vec<_> = ids
        .iter()
        .map(|id| serde_json::json!({
            "identity":{"accountId": id}, "auth":{"type":"oauth","accessToken":id}, "proxyIndex":0
        }))
        .collect();
    let bundle = serde_json::json!({
        "format":"zenith", "version":2, "proxies":[url.as_str()], "accounts":accounts
    });
    preview_account_import_documents(vec![bundle.to_string()], state)
        .await
        .unwrap()
}

fn input(
    session: &ImportSessionResponse,
    count: usize,
    choice: Option<bool>,
) -> ConfirmAccountImportInput {
    ConfirmAccountImportInput {
        session_id: session.session_id.clone(),
        selected_item_ids: session.preview.rows[..count]
            .iter()
            .map(|r| r.item_id.clone())
            .collect(),
        add_to_pool: false,
        use_bundle_proxies: choice,
        discover_models: false,
        probe_quota: false,
        models: Vec::new(),
    }
}

async fn confirm(
    state: &DesktopState,
    session: &ImportSessionResponse,
    count: usize,
    choice: bool,
) {
    let result =
        confirm_local_account_import_inner(input(session, count, Some(choice)), state, None)
            .await
            .unwrap();
    assert_eq!(result.results.len(), count);
    for row in result.results {
        assert_eq!(row.status, ImportItemStatus::Succeeded);
    }
}

#[tokio::test]
async fn file_proxies_require_confirmation_and_preserve_existing_routes() {
    let root = crate::files::test_temp_root("account-proxy-transfer");
    let mut state = DesktopState::open(root.clone()).unwrap();
    let (direct, direct_hits, direct_server) = counting_endpoint().await;
    let (mut route, proxy_hits, proxy_server) = counting_endpoint().await;
    route.set_username("fixture-user").unwrap();
    route.set_password(Some("fixture-password")).unwrap();
    state.set_account_check_url_for_test(direct.join("accounts/check").unwrap());
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let session = preview(
        &state,
        &["fixture-first", "fixture-second", "fixture-unselected"],
        &route,
    )
    .await;
    assert!(
        confirm_local_account_import_inner(input(&session, 1, None), &state, None)
            .await
            .is_err()
    );
    assert_eq!(proxy_hits.load(Ordering::SeqCst), 0);
    assert_eq!(direct_hits.load(Ordering::SeqCst), 0);
    assert!(state.store().unwrap().accounts().is_empty());
    confirm(&state, &session, 2, true).await;
    assert_eq!(proxy_hits.load(Ordering::SeqCst), 2);
    assert_eq!(direct_hits.load(Ordering::SeqCst), 0);
    let ids: Vec<_> = state
        .store()
        .unwrap()
        .accounts()
        .iter()
        .map(|a| a.account.id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    let bindings: Vec<_> = ids
        .iter()
        .map(|id| {
            let saved = credentials.require(id).unwrap();
            assert_eq!(saved.proxy_url(), Some(route.as_str()));
            (id.clone(), saved.proxy_url().map(str::to_owned))
        })
        .collect();
    let mut pool = proxy::ProxyPool::default();
    assert!(pool.reconcile(&bindings, 1));
    assert_eq!(pool.summary().total, 1);
    assert_eq!(pool.summary().entries[0].assigned_account_ids.len(), 2);
    for include in [false, true] {
        let export = build_local_account_export_document(
            &ids,
            AccountExportFormat::Zenith,
            None,
            include,
            &state,
        )
        .unwrap();
        assert_eq!(export.content.contains("fixture-password"), include);
    }
    // A known account keeps its saved route, even when the newly selected file uses another.
    let other = preview(&state, &["fixture-first"], &direct).await;
    assert!(other.preview.rows[0].existing);
    assert!(!other.preview.rows[0].default_selected);
    confirm(&state, &other, 1, true).await;
    assert_eq!(proxy_hits.load(Ordering::SeqCst), 3);
    let local = preview(&state, &["fixture-local"], &route).await;
    confirm(&state, &local, 1, false).await;
    assert_eq!(direct_hits.load(Ordering::SeqCst), 1);
    assert_eq!(proxy_hits.load(Ordering::SeqCst), 3);
    for account in state.store().unwrap().accounts() {
        let id = &account.account.id;
        assert_eq!(
            credentials.require(id).unwrap().proxy_url().is_some(),
            ids.contains(id)
        );
        credentials.delete(id).unwrap();
    }
    direct_server.abort();
    proxy_server.abort();
    drop(state);
    fs::remove_dir_all(root).unwrap();
}
