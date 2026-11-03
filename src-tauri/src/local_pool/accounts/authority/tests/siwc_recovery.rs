use super::*;
use crate::local_pool::accounts::oauth::{
    CodexOAuthClient, OAuthClientKind,
    siwc::{PendingSiwcRefresh, SiwcRegistration},
};
use serde_json::json;

fn initial() -> StoredCodexCredentials {
    let registration = SiwcRegistration {
        client_id: "oaiapp_synthetic".into(),
        host_id: "urn:uuid:synthetic-host".into(),
        issuer: "https://auth.openai.com".into(),
        subject: "synthetic-subject".into(),
        scopes: vec!["resource.invoke".into(), "chatgpt.tokens.use.direct".into()],
        earliest_refresh_at_ms: None,
        requires_reauthentication: false,
    };
    StoredCodexCredentials::new(
        "siwc_recovery",
        "old-access".into(),
        Some("old-refresh".into()),
        None,
        Some(10),
        1,
        1,
        None,
        Some(registration.identity_key()),
        Some(registration.subject.clone()),
        None,
        None,
        false,
    )
    .unwrap()
    .with_oauth_client_kind(OAuthClientKind::Siwc)
    .with_siwc_registration(Some(registration))
    .unwrap()
}

fn pending(before: &StoredCodexCredentials, now_ms: u64) -> PendingSiwcRefresh {
    serde_json::from_value(json!({
        "registration": before.siwc_registration(), "access_token":"rotated-access",
        "refresh_token":"rotated-refresh", "id_token":null,
        "expires_at_ms":now_ms + 3_600_000, "received_at_ms":now_ms,
    }))
    .unwrap()
}

struct Client {
    store: CredentialStore<MemorySecrets>,
    received: AtomicUsize,
    verified: AtomicUsize,
    fail: AtomicBool,
    pause: AtomicBool,
    verifying: tokio::sync::Notify,
}

impl CodexRefreshClient for Client {
    fn refresh_siwc<'a>(
        &'a self,
        account: &'a str,
        _: &'a SiwcRegistration,
        token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<PendingSiwcRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            assert_eq!(token, "old-refresh");
            assert_eq!(
                self.received.fetch_add(1, Ordering::SeqCst),
                0,
                "consumed grant was resent"
            );
            Ok(pending(&self.store.require(account).unwrap(), now_ms))
        })
    }
    fn verify_siwc_refresh<'a>(
        &'a self,
        account: &'a str,
        pending: PendingSiwcRefresh,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            self.verified.fetch_add(1, Ordering::SeqCst);
            let current = self.store.require(account).unwrap();
            assert_eq!(current.access_token(), "old-access");
            assert!(
                self.store
                    .pending_siwc_rotation(&current)
                    .unwrap()
                    .is_some()
            );
            self.verifying.notify_one();
            if self.pause.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.fail.load(Ordering::SeqCst) {
                return Err(TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "siwc_identity_verification_unavailable",
                ));
            }
            let tokens = CodexOAuthClient::new()
                .unwrap()
                .verify_pending_siwc_refresh(pending, now_ms)
                .await?;
            Ok(CredentialRefresh::from_oauth(tokens).unwrap())
        })
    }
    fn refresh<'a>(
        &'a self,
        _: &'a str,
        _: Option<&'a str>,
        _: &'a str,
        _: u64,
        _: OAuthClientKind,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async { panic!("SIWC must not use native OAuth") })
    }
}

#[tokio::test]
async fn siwc_received_rotation_survives_verification_outage_and_restart() {
    exercise_recovery(false).await;
}

#[tokio::test]
async fn siwc_invalid_received_identity_requires_reauthentication_without_a_request() {
    let mut value = serde_json::to_value(pending(&initial(), 1000)).unwrap();
    value["id_token"] = json!("synthetic-invalid-jwt");
    let received = serde_json::from_value(value).unwrap();
    let error = CodexOAuthClient::new()
        .unwrap()
        .verify_pending_siwc_refresh(received, 1001)
        .await
        .unwrap_err();
    assert_eq!(
        error.kind,
        TokenRefreshFailureKind::RotationRecoveryRequired
    );
    assert_eq!(error.code, "siwc_rotation_identity_invalid");
    assert!(!format!("{error:?}").contains("synthetic-invalid-jwt"));
}

#[test]
fn siwc_received_rotation_cannot_change_registration_or_subject() {
    let store = CredentialStore::from_backend(MemorySecrets::default());
    let before = initial();
    store.save(&before).unwrap();
    let original = serde_json::to_value(pending(&before, 1000)).unwrap();
    for (field, value) in [
        ("clientId", "oaiapp_foreign"),
        ("hostId", "urn:uuid:foreign-host"),
        ("subject", "foreign-subject"),
        ("issuer", "https://foreign.invalid"),
    ] {
        let mut changed = original.clone();
        changed["registration"][field] = json!(value);
        let received = serde_json::from_value(changed).unwrap();
        assert!(
            store
                .checkpoint_pending_siwc_rotation(&before, &received)
                .is_err()
        );
        assert!(
            store
                .require("siwc_recovery")
                .unwrap()
                .matches_snapshot(&before)
        );
    }
}

#[tokio::test]
async fn siwc_received_rotation_survives_cancellation_before_verification() {
    exercise_recovery(true).await;
}

async fn exercise_recovery(cancel: bool) {
    let root = temp_root("siwc-pending");
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend.clone());
    let before = initial();
    store.save(&before).unwrap();
    let client = Arc::new(Client {
        store: store.clone(),
        received: AtomicUsize::new(0),
        verified: AtomicUsize::new(0),
        fail: AtomicBool::new(!cancel),
        pause: AtomicBool::new(cancel),
        verifying: tokio::sync::Notify::new(),
    });
    let adapter = Arc::new(
        StoredRefreshAdapter::with_lock_config(
            root.clone(),
            store.clone(),
            client.clone(),
            0,
            fast_lock_config(),
        )
        .unwrap(),
    );
    if cancel {
        let active = adapter.clone();
        let task =
            tokio::spawn(async move { active.refresh("siwc_recovery", "old-refresh", 1000).await });
        tokio::time::timeout(Duration::from_secs(5), client.verifying.notified())
            .await
            .unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    } else {
        assert_eq!(
            adapter
                .refresh("siwc_recovery", "old-refresh", 1000)
                .await
                .unwrap_err()
                .code,
            "siwc_identity_verification_unavailable"
        );
    }
    drop(adapter);
    assert_eq!(
        store.require("siwc_recovery").unwrap().access_token(),
        "old-access"
    );
    client.fail.store(false, Ordering::SeqCst);
    client.pause.store(false, Ordering::SeqCst);
    let restarted = StoredRefreshAdapter::with_lock_config(
        root.clone(),
        CredentialStore::new(backend),
        client.clone(),
        0,
        fast_lock_config(),
    )
    .unwrap();
    restarted
        .refresh("siwc_recovery", "old-refresh", 10_000)
        .await
        .unwrap();
    let saved = store.require("siwc_recovery").unwrap();
    assert_eq!(saved.access_token(), "rotated-access");
    assert_eq!(saved.refresh_token(), Some("rotated-refresh"));
    assert_eq!(saved.generation(), 2);
    assert_eq!(saved.expires_at_ms(), Some(3_601_000));
    assert!(store.pending_siwc_rotation(&saved).unwrap().is_none());
    assert_eq!(client.received.load(Ordering::SeqCst), 1);
    assert_eq!(client.verified.load(Ordering::SeqCst), 2);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn siwc_pending_checkpoint_is_not_authorization_and_new_login_wins() {
    let store = CredentialStore::from_backend(MemorySecrets::default());
    let before = initial();
    store.save(&before).unwrap();
    let pending = pending(&before, 1000);
    assert!(!format!("{pending:?}").contains("rotated-refresh"));
    store
        .checkpoint_pending_siwc_rotation(&before, &pending)
        .unwrap();
    assert!(store.recover_token_rotation(&before).is_err());
    let tokens = zenith_relay_core::accounts::TokenSet::new(
        "new-login",
        Some("new-login-refresh".into()),
        None,
        Some(60_000),
        2000,
        2,
    )
    .unwrap();
    let newer = before.with_token_set(&tokens).unwrap();
    store.save(&newer).unwrap();
    assert!(store.pending_siwc_rotation(&newer).unwrap().is_none());
    assert!(
        store
            .require("siwc_recovery")
            .unwrap()
            .matches_snapshot(&newer)
    );
}
