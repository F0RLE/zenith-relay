use super::*;
use crate::local_pool::accounts::oauth::OAuthClientKind;
use zenith_relay_core::accounts::ReauthReason;

#[derive(Clone, Copy)]
enum Outage {
    PrimaryWrite,
    AllWrites,
    Reads,
}

struct RotateThenFailStorage {
    backend: Arc<MemorySecrets>,
    calls: AtomicUsize,
    outage: Outage,
}

impl CodexRefreshClient for RotateThenFailStorage {
    fn refresh<'a>(
        &'a self,
        _local_account_id: &'a str,
        _provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
        _kind: OAuthClientKind,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            assert_eq!(refresh_token, "old-refresh-secret");
            assert_eq!(
                self.calls.fetch_add(1, Ordering::SeqCst),
                0,
                "old grant was replayed"
            );
            match self.outage {
                Outage::PrimaryWrite => {
                    self.backend.fail_primary_save.store(true, Ordering::SeqCst)
                }
                Outage::AllWrites => self.backend.fail_save.store(true, Ordering::SeqCst),
                Outage::Reads => self.backend.fail_load.store(true, Ordering::SeqCst),
            }
            Ok(CredentialRefresh::new(
                "rotated-access".into(),
                Some("rotated-refresh".into()),
                Some("rotated-id".into()),
                Some(now_ms + 600_000),
            )
            .unwrap())
        })
    }
}

fn restore_storage(backend: &MemorySecrets) {
    backend.fail_primary_save.store(false, Ordering::SeqCst);
    backend.fail_save.store(false, Ordering::SeqCst);
    backend.fail_load.store(false, Ordering::SeqCst);
}

#[tokio::test]
async fn successful_native_rotation_is_retained_until_persistence_can_retry() {
    for kind in [OAuthClientKind::Codex, OAuthClientKind::ExcelBps] {
        for outage in [Outage::PrimaryWrite, Outage::AllWrites, Outage::Reads] {
            let root = temp_root("native-rotation-pending");
            let backend = Arc::new(MemorySecrets::default());
            let store = CredentialStore::new(backend.clone());
            let initial = expired_credentials().with_oauth_client_kind(kind);
            store.save(&initial).unwrap();
            let client = Arc::new(RotateThenFailStorage {
                backend: backend.clone(),
                calls: AtomicUsize::new(0),
                outage,
            });
            let adapter =
                StoredRefreshAdapter::new(root.clone(), store.clone(), client.clone(), 0).unwrap();
            let persistence = CredentialPersistence::new(
                store.clone(),
                Arc::new(CaptureMetadata::default()),
                root.clone(),
            );
            let authority = TokenAuthority::new(1).unwrap();
            authority
                .register(
                    "relay_account_1",
                    initial.to_token_set().unwrap(),
                    AccountAuthState::Active,
                )
                .await
                .unwrap();
            assert!(matches!(
                authority
                    .prepare_and_persist("relay_account_1", 10, 0, &adapter, &persistence)
                    .await,
                Err(TokenAuthorityError::PersistenceFailed(_))
            ));
            assert_eq!(
                authority
                    .tokens("relay_account_1")
                    .await
                    .unwrap()
                    .refresh_token(),
                Some("rotated-refresh")
            );

            restore_storage(&backend);
            authority
                .register_if_newer(
                    "relay_account_1",
                    initial.to_token_set().unwrap(),
                    AccountAuthState::Active,
                )
                .await
                .unwrap();
            // Request preparation creates new adapter instances. Pending tokens
            // belong to the shared authority, not the discarded adapter.
            drop(adapter);
            let adapter =
                StoredRefreshAdapter::new(root.clone(), store.clone(), client.clone(), 0).unwrap();
            let prepared = authority
                .prepare_and_persist("relay_account_1", 11, 0, &adapter, &persistence)
                .await
                .unwrap();
            assert_eq!(prepared.status, PrepareStatus::Ready);
            assert_eq!(prepared.tokens.refresh_token(), Some("rotated-refresh"));
            let saved = store.require("relay_account_1").unwrap();
            assert_eq!(saved.oauth_client_kind(), kind);
            assert_eq!(saved.generation(), initial.generation() + 1);
            assert_eq!(saved.to_token_set().unwrap(), prepared.tokens);
            assert!(store.recover_token_rotation(&saved).unwrap().is_none());
            assert_eq!(client.calls.load(Ordering::SeqCst), 1);
            fs::remove_dir_all(root).unwrap();
        }
    }
}

#[tokio::test]
async fn native_rotation_checkpoint_recovers_after_restart_without_exchanging_again() {
    for kind in [OAuthClientKind::Codex, OAuthClientKind::ExcelBps] {
        let root = temp_root("native-rotation-restart");
        let backend = Arc::new(MemorySecrets::default());
        let store = CredentialStore::new(backend.clone());
        let initial = expired_credentials().with_oauth_client_kind(kind);
        store.save(&initial).unwrap();
        let client = Arc::new(RotateThenFailStorage {
            backend: backend.clone(),
            calls: AtomicUsize::new(0),
            outage: Outage::PrimaryWrite,
        });
        let adapter =
            StoredRefreshAdapter::new(root.clone(), store.clone(), client.clone(), 0).unwrap();
        let authority = TokenAuthority::new(1).unwrap();
        authority
            .register(
                "relay_account_1",
                initial.to_token_set().unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let persistence = CredentialPersistence::new(
            store.clone(),
            Arc::new(CaptureMetadata::default()),
            root.clone(),
        );
        assert!(matches!(
            authority
                .prepare_and_persist("relay_account_1", 10, 0, &adapter, &persistence)
                .await,
            Err(TokenAuthorityError::PersistenceFailed(_))
        ));
        drop(authority);
        drop(adapter);
        restore_storage(&backend);

        let restarted = TokenAuthority::new(1).unwrap();
        restarted
            .register(
                "relay_account_1",
                initial.to_token_set().unwrap(),
                AccountAuthState::Active,
            )
            .await
            .unwrap();
        let adapter =
            StoredRefreshAdapter::new(root.clone(), store.clone(), client.clone(), 0).unwrap();
        let prepared = restarted
            .prepare_and_persist("relay_account_1", 11, 0, &adapter, &persistence)
            .await
            .unwrap();
        assert_eq!(prepared.tokens.refresh_token(), Some("rotated-refresh"));
        assert_eq!(client.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            store
                .require("relay_account_1")
                .unwrap()
                .to_token_set()
                .unwrap(),
            prepared.tokens
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn total_storage_outage_leaves_a_restart_barrier_instead_of_replaying_the_grant() {
    let root = temp_root("native-rotation-barrier");
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend.clone());
    let initial = expired_credentials();
    store.save(&initial).unwrap();
    let client = Arc::new(RotateThenFailStorage {
        backend: backend.clone(),
        calls: AtomicUsize::new(0),
        outage: Outage::AllWrites,
    });
    let adapter =
        StoredRefreshAdapter::new(root.clone(), store.clone(), client.clone(), 0).unwrap();
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "relay_account_1",
            initial.to_token_set().unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let persistence = CredentialPersistence::new(
        store.clone(),
        Arc::new(CaptureMetadata::default()),
        root.clone(),
    );
    assert!(matches!(
        authority
            .prepare_and_persist("relay_account_1", 10, 0, &adapter, &persistence)
            .await,
        Err(TokenAuthorityError::PersistenceFailed(_))
    ));
    drop(authority);
    restore_storage(&backend);
    let restarted = TokenAuthority::new(1).unwrap();
    restarted
        .register(
            "relay_account_1",
            initial.to_token_set().unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    assert_eq!(
        restarted
            .prepare_and_persist("relay_account_1", 11, 0, &adapter, &persistence)
            .await
            .unwrap_err(),
        TokenAuthorityError::RequiresReauth(ReauthReason::TokenRotationRecoveryRequired),
    );
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    fs::remove_dir_all(root).unwrap();
}
