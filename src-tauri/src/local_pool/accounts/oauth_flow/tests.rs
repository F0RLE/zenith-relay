use super::super::import_session::SecretBackendError;
use super::callback::MAX_REQUEST_HEADER_BYTES;
use super::callback::{callback_language, callback_success_html, CallbackLanguage};
use super::snapshot::validate_snapshot;
use super::snapshot::{ensure_pending_directory, pending_directory};
use super::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::Url;

static TEST_OAUTH_PORT_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Clone, Default)]
struct MemorySecrets(Arc<Mutex<BTreeMap<String, String>>>);

impl SecretBackend for MemorySecrets {
    fn save(&self, secret_ref: &str, value: &str) -> Result<(), SecretBackendError> {
        lock(&self.0).insert(secret_ref.to_string(), value.to_string());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
        Ok(lock(&self.0).get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
        lock(&self.0).remove(secret_ref);
        Ok(())
    }
}

impl MemorySecrets {
    fn contains(&self, secret_ref: &str) -> bool {
        lock(&self.0).contains_key(secret_ref)
    }
}

#[derive(Clone, Default)]
struct Events(Arc<Mutex<Vec<OAuthFlowEvent>>>);

impl OAuthFlowEventSink for Events {
    fn emit(&self, event: OAuthFlowEvent) {
        lock(&self.0).push(event);
    }
}

impl Events {
    fn has(&self, login_id: &str, status: OAuthFlowStatus) -> bool {
        lock(&self.0)
            .iter()
            .any(|event| event.login_id == login_id && event.status == status)
    }
}

#[tokio::test]
async fn loopback_callback_validates_state_and_stores_only_secret_material() {
    let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
    let root = test_root("callback-success");
    let secrets = MemorySecrets::default();
    let events = Events::default();
    let manager = OAuthFlowManager::new(root.clone(), secrets.clone(), events.clone());
    let start = manager
        .start_for_account(&CodexOAuthClient::new().unwrap(), None, None)
        .await
        .unwrap();
    assert!(CODEX_OAUTH_CALLBACK_PORTS
        .contains(&Url::parse(&start.redirect_uri).unwrap().port().unwrap()));
    let callback = callback_url_for(&start, "authorization-code", None);

    let response = send_callback(&start.redirect_uri, &request_target(&callback)).await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response.contains("Content-Type: text/html; charset=utf-8"));
    assert!(response.contains("text-align:center"));
    assert!(response.contains("You can close this window now."));
    assert!(response.contains("user-select:none"));
    assert!(response.contains("-webkit-user-select:none"));
    assert!(!response.contains("<button"));
    assert!(!response.contains("<script"));
    assert!(!response.contains("authorization-code"));
    wait_until(|| events.has(&start.login_id, OAuthFlowStatus::CallbackReceived)).await;
    assert!(secrets.contains(&callback_secret_ref(&start.login_id)));
    let material = manager.exchange_material(&start.login_id).unwrap();
    assert!(!format!("{material:?}").contains("authorization-code"));
    let (pending, callback) = material.into_parts();
    assert_eq!(pending.redirect_uri(), start.redirect_uri);
    assert!(!format!("{pending:?} {callback:?}").contains("authorization-code"));
    let snapshot = fs::read_to_string(snapshot_path(&root, &start.login_id).unwrap()).unwrap();
    assert!(!snapshot.contains("authorization-code"));
    assert!(!snapshot.contains("access-token"));
    assert!(!format!("{manager:?} {start:?}").contains("authorization-code"));
    manager.complete(&start.login_id).await.unwrap();
    remove_root(&root);
}

#[test]
fn callback_success_page_uses_supported_browser_language() {
    assert_eq!(
        callback_language("GET / HTTP/1.1\r\nAccept-Language: ru-RU,ru;q=0.9,en;q=0.8"),
        CallbackLanguage::Russian
    );
    assert_eq!(
        callback_language("GET / HTTP/1.1\r\nAccept-Language: de-DE,de;q=0.9,en;q=0.8"),
        CallbackLanguage::English
    );
    assert!(
        callback_success_html(CallbackLanguage::Russian).contains("Теперь это окно можно закрыть.")
    );
    assert!(
        callback_success_html(CallbackLanguage::English).contains("You can close this window now.")
    );
}

#[tokio::test]
async fn state_mismatch_is_rejected_and_listener_remains_cancelable() {
    let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
    let root = test_root("state-mismatch");
    let secrets = MemorySecrets::default();
    let events = Events::default();
    let manager = OAuthFlowManager::new(root.clone(), secrets.clone(), events.clone());
    let start = manager
        .start_for_account(&CodexOAuthClient::new().unwrap(), None, None)
        .await
        .unwrap();
    let callback = callback_url_for(&start, "authorization-code", Some("wrong-state"));

    let response = send_callback(&start.redirect_uri, &request_target(&callback)).await;
    assert!(response.starts_with("HTTP/1.1 400"));
    wait_until(|| events.has(&start.login_id, OAuthFlowStatus::CallbackRejected)).await;
    assert!(!secrets.contains(&callback_secret_ref(&start.login_id)));
    assert_eq!(
        manager.status(&start.login_id).unwrap().status,
        OAuthFlowStatus::Pending
    );
    manager.cancel(&start.login_id).await.unwrap();
    remove_root(&root);
}

#[tokio::test]
async fn manual_callback_and_restart_resume_use_the_known_pending_session() {
    let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
    let root = test_root("manual-restart");
    let secrets = MemorySecrets::default();
    let first_events = Events::default();
    let first = OAuthFlowManager::new(root.clone(), secrets.clone(), first_events);
    let start = first
        .start_for_account(&CodexOAuthClient::new().unwrap(), None, None)
        .await
        .unwrap();
    first.shutdown().await;

    let second_events = Events::default();
    let second = OAuthFlowManager::new(root.clone(), secrets.clone(), second_events.clone());
    let resumed = second.resume(&start.login_id).await.unwrap();
    assert_eq!(resumed.redirect_uri, start.redirect_uri);
    let callback = callback_url_for(&resumed, "manual-code", None);
    second
        .submit_manual_callback(&resumed.login_id, callback.as_str())
        .await
        .unwrap();
    assert_eq!(
        second.status(&resumed.login_id).unwrap().status,
        OAuthFlowStatus::CallbackReceived
    );
    assert!(second_events.has(&resumed.login_id, OAuthFlowStatus::CallbackReceived));
    second.complete(&resumed.login_id).await.unwrap();
    remove_root(&root);
}

#[tokio::test]
async fn targeted_reauth_survives_oauth_flow_resume() {
    let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
    let root = test_root("targeted-reauth");
    let manager = OAuthFlowManager::new(root.clone(), MemorySecrets::default(), Events::default());
    let start = manager
        .start_for_account(
            &CodexOAuthClient::new().unwrap(),
            Some("account_local"),
            None,
        )
        .await
        .unwrap();
    assert_eq!(start.target_account_id.as_deref(), Some("account_local"));
    manager.shutdown().await;

    let resumed = manager.resume(&start.login_id).await.unwrap();
    assert_eq!(resumed.target_account_id.as_deref(), Some("account_local"));
    manager.cancel(&start.login_id).await.unwrap();
    remove_root(&root);
}

#[tokio::test]
async fn cancel_removes_snapshot_and_releases_callback_port() {
    let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
    let root = test_root("cancel");
    let manager = OAuthFlowManager::new(root.clone(), MemorySecrets::default(), Events::default());
    let start = manager
        .start_for_account(&CodexOAuthClient::new().unwrap(), None, None)
        .await
        .unwrap();
    let port = Url::parse(&start.redirect_uri).unwrap().port().unwrap();
    manager.cancel(&start.login_id).await.unwrap();

    assert!(!snapshot_path(&root, &start.login_id).unwrap().exists());
    let rebound = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    drop(rebound);
    remove_root(&root);
}

#[tokio::test]
async fn traversal_oversized_requests_and_corrupt_snapshots_fail_safely() {
    let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
    let root = test_root("unsafe");
    let manager = OAuthFlowManager::new(root.clone(), MemorySecrets::default(), Events::default());
    assert_eq!(
        manager.resume("../oauth.json").await.unwrap_err().code,
        OAuthFlowErrorCode::InvalidLoginId
    );

    let start = manager
        .start_for_account(&CodexOAuthClient::new().unwrap(), None, None)
        .await
        .unwrap();
    let oversized = format!(
        "GET /auth/callback?{} HTTP/1.1\r\nHost: localhost\r\n\r\n",
        "x".repeat(MAX_REQUEST_HEADER_BYTES)
    );
    let response = send_raw(&start.redirect_uri, &oversized).await;
    assert!(response.starts_with("HTTP/1.1 413"));
    assert_eq!(
        manager.status(&start.login_id).unwrap().status,
        OAuthFlowStatus::Pending
    );
    manager.cancel(&start.login_id).await.unwrap();

    let corrupt_id = Uuid::new_v4().hyphenated().to_string();
    ensure_pending_directory(&pending_directory(&root)).unwrap();
    fs::write(
        snapshot_path(&root, &corrupt_id).unwrap(),
        r#"{"version":1,"authorizationCode":"raw-code"}"#,
    )
    .unwrap();
    let error = manager.resume(&corrupt_id).await.unwrap_err();
    assert_eq!(error.code, OAuthFlowErrorCode::RecoveryRequired);
    assert!(!format!("{error:?} {error}").contains("raw-code"));
    remove_root(&root);
}

#[test]
fn persisted_authorization_url_is_strictly_validated() {
    let login_id = Uuid::new_v4().hyphenated().to_string();
    let oauth_start = CodexOAuthClient::new()
        .unwrap()
        .begin(1455, 10_000)
        .unwrap();
    let authorization_url = oauth_start.authorization_url().to_string();
    let pending = oauth_start.into_pending();
    let snapshot = PendingSnapshot {
        version: SNAPSHOT_VERSION,
        login_id: login_id.clone(),
        authorization_url,
        callback_secret_ref: callback_secret_ref(&login_id),
        status: OAuthFlowStatus::Pending,
        target_account_id: None,
        sign_in_proxy_id: None,
        pending,
    };
    validate_snapshot(&snapshot, &login_id).unwrap();
    let mut bad_proxy = snapshot.clone();
    bad_proxy.sign_in_proxy_id = Some("not a proxy".to_string());
    assert_eq!(
        validate_snapshot(&bad_proxy, &login_id).unwrap_err().code,
        OAuthFlowErrorCode::RecoveryRequired
    );

    let redirect_uri = snapshot.pending.redirect_uri();
    let mut wrong_host = Url::parse("https://attacker.invalid/oauth/authorize").unwrap();
    wrong_host
        .query_pairs_mut()
        .append_pair("redirect_uri", redirect_uri);
    let mut credentials = Url::parse("https://user:pass@auth.openai.com/oauth/authorize").unwrap();
    credentials
        .query_pairs_mut()
        .append_pair("redirect_uri", redirect_uri);
    let mut fragment = Url::parse(AUTHORIZATION_ENDPOINT).unwrap();
    fragment
        .query_pairs_mut()
        .append_pair("redirect_uri", redirect_uri);
    fragment.set_fragment(Some("callback"));
    let mut sensitive = Url::parse(AUTHORIZATION_ENDPOINT).unwrap();
    sensitive
        .query_pairs_mut()
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("access_token", "secret");
    let mut wrong_redirect = Url::parse(AUTHORIZATION_ENDPOINT).unwrap();
    wrong_redirect
        .query_pairs_mut()
        .append_pair("redirect_uri", "http://localhost:9999/auth/callback");
    let mut duplicate_redirect = Url::parse(AUTHORIZATION_ENDPOINT).unwrap();
    duplicate_redirect
        .query_pairs_mut()
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("redirect_uri", redirect_uri);

    for authorization_url in [
        wrong_host,
        credentials,
        fragment,
        sensitive,
        wrong_redirect,
        duplicate_redirect,
    ] {
        let mut tampered = snapshot.clone();
        tampered.authorization_url = authorization_url.to_string();
        assert_eq!(
            validate_snapshot(&tampered, &login_id).unwrap_err().code,
            OAuthFlowErrorCode::RecoveryRequired
        );
    }
}

#[tokio::test]
async fn exchange_material_requires_received_callback_and_redacts_all_secrets() {
    let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
    let root = test_root("exchange-material");
    let manager = OAuthFlowManager::new(root.clone(), MemorySecrets::default(), Events::default());
    let start = manager
        .start_for_account(&CodexOAuthClient::new().unwrap(), None, None)
        .await
        .unwrap();
    assert_eq!(
        manager.exchange_material(&start.login_id).unwrap_err().code,
        OAuthFlowErrorCode::SecretMissing
    );
    let callback = callback_url_for(&start, "exchange-secret", None);
    manager
        .submit_manual_callback(&start.login_id, callback.as_str())
        .await
        .unwrap();

    let material = manager.exchange_material(&start.login_id).unwrap();
    assert_eq!(
        format!("{material:?}"),
        "OAuthExchangeMaterial { pending: \"[redacted]\", callback: \"[redacted]\" }"
    );
    let (pending, callback) = material.into_parts();
    assert_eq!(pending.redirect_uri(), start.redirect_uri);
    assert!(!format!("{pending:?} {callback:?}").contains("exchange-secret"));

    manager.complete(&start.login_id).await.unwrap();
    remove_root(&root);
}

#[tokio::test]
async fn pending_sign_in_proxy_can_change_until_the_callback_arrives() {
    let _port_guard = TEST_OAUTH_PORT_LOCK.lock().await;
    let root = test_root("sign-in-proxy");
    let manager = OAuthFlowManager::new(root.clone(), MemorySecrets::default(), Events::default());
    let oauth = CodexOAuthClient::new().unwrap();
    let start = manager
        .start_for_account(&oauth, None, Some("proxy_one"))
        .await
        .unwrap();
    assert_eq!(
        manager
            .sign_in_proxy_id(&start.login_id)
            .unwrap()
            .as_deref(),
        Some("proxy_one")
    );

    let again = manager
        .start_for_account(&oauth, None, Some("proxy_two"))
        .await
        .unwrap();
    assert_eq!(again.login_id, start.login_id);
    assert_eq!(
        manager
            .sign_in_proxy_id(&start.login_id)
            .unwrap()
            .as_deref(),
        Some("proxy_two")
    );

    let callback = callback_url_for(&start, "proxy-code", None);
    manager
        .submit_manual_callback(&start.login_id, callback.as_str())
        .await
        .unwrap();
    let locked = manager
        .start_for_account(&oauth, None, Some("proxy_three"))
        .await
        .unwrap();
    assert_eq!(locked.login_id, start.login_id);
    assert_eq!(locked.status, OAuthFlowStatus::CallbackReceived);
    assert_eq!(
        manager
            .sign_in_proxy_id(&start.login_id)
            .unwrap()
            .as_deref(),
        Some("proxy_two")
    );

    manager.complete(&start.login_id).await.unwrap();
    remove_root(&root);
}

fn callback_url_for(start: &OAuthFlowStart, code: &str, state_override: Option<&str>) -> Url {
    let authorization = Url::parse(&start.authorization_url).unwrap();
    let state = state_override
        .map(str::to_string)
        .or_else(|| {
            authorization
                .query_pairs()
                .find_map(|(key, value)| (key == "state").then(|| value.into_owned()))
        })
        .unwrap();
    let mut callback = Url::parse(&start.redirect_uri).unwrap();
    callback
        .query_pairs_mut()
        .append_pair("code", code)
        .append_pair("state", &state);
    callback
}

fn request_target(url: &Url) -> String {
    url.query().map_or_else(
        || url.path().to_string(),
        |query| format!("{}?{query}", url.path()),
    )
}

async fn send_callback(redirect_uri: &str, path_and_query: &str) -> String {
    let request = format!(
        "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
        path_and_query
    );
    send_raw(redirect_uri, &request).await
}

async fn send_raw(redirect_uri: &str, request: &str) -> String {
    let port = Url::parse(redirect_uri).unwrap().port().unwrap();
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8(response).unwrap()
}

async fn wait_until(condition: impl Fn() -> bool) {
    for _ in 0..100 {
        if condition() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("condition was not reached");
}

fn test_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-oauth-flow-{label}-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}

fn remove_root(root: &Path) {
    let _ = fs::remove_dir_all(root);
}
