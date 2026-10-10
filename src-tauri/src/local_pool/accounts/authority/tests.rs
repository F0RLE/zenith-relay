use super::super::credentials::{CredentialRefresh, CredentialStore};
use super::super::{
    credentials::StoredCodexCredentials,
    import_session::{SecretBackend, SecretBackendError},
};
use super::lock::{ensure_lock_dir, lock_path, LockOwner};
use super::*;
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Mutex,
    },
};
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;
use zenith_relay_core::accounts::{
    AccountAuthState, PrepareStatus, TokenAuthority, TokenAuthorityError, TokenPersistenceAdapter,
    TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind,
};
use zenith_relay_core::providers::chatgpt::AgentIdentityCredential;
use zenith_relay_core::unix_time_ms as now_ms;

#[derive(Default)]
struct MemorySecrets {
    values: Mutex<HashMap<String, String>>,
    fail_save: AtomicBool,
}

impl SecretBackend for MemorySecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<(), SecretBackendError> {
        if self.fail_save.load(Ordering::SeqCst) {
            return Err(SecretBackendError);
        }
        self.values
            .lock()
            .unwrap()
            .insert(secret_ref.into(), secret_value.into());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
        Ok(self.values.lock().unwrap().get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
        self.values.lock().unwrap().remove(secret_ref);
        Ok(())
    }
}

struct RefreshOnce {
    calls: AtomicUsize,
}

impl CodexRefreshClient for RefreshOnce {
    fn refresh<'a>(
        &'a self,
        _local_account_id: &'a str,
        provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
        _kind: super::super::oauth::OAuthClientKind,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            assert_eq!(provider_account_id, Some("provider-private-id"));
            assert_eq!(refresh_token, "old-refresh-secret");
            self.calls.fetch_add(1, Ordering::SeqCst);
            sleep(Duration::from_millis(20)).await;
            CredentialRefresh::new(
                "new-access-secret".into(),
                Some("new-refresh-secret".into()),
                Some("new-id-secret".into()),
                Some(now_ms + 60_000),
            )
            .map_err(|_| {
                TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, "invalid_fixture")
            })
        })
    }
}

#[derive(Default)]
struct CaptureMetadata {
    generation_calls: AtomicUsize,
    fail_generation: AtomicBool,
    generations: Mutex<Vec<(String, u64, u64)>>,
    auth_states: Mutex<Vec<(String, AccountAuthState)>>,
}

impl AccountMetadataSink for CaptureMetadata {
    fn persist_generation<'a>(
        &'a self,
        local_account_id: &'a str,
        generation: u64,
        updated_at_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetadataSinkError>> + Send + 'a>> {
        Box::pin(async move {
            self.generation_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_generation.load(Ordering::SeqCst) {
                return Err(MetadataSinkError);
            }
            self.generations.lock().unwrap().push((
                local_account_id.to_string(),
                generation,
                updated_at_ms,
            ));
            Ok(())
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        local_account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetadataSinkError>> + Send + 'a>> {
        Box::pin(async move {
            self.auth_states
                .lock()
                .unwrap()
                .push((local_account_id.to_string(), auth_state));
            Ok(())
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn twenty_concurrent_refreshes_make_one_network_rotation() {
    let root = temp_root("concurrent");
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend);
    store.save(&expired_credentials()).unwrap();
    let client = Arc::new(RefreshOnce {
        calls: AtomicUsize::new(0),
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
    let mut tasks = Vec::new();
    for _ in 0..20 {
        let adapter = adapter.clone();
        tasks.push(tokio::spawn(async move {
            adapter
                .refresh("relay_account_1", "stale-refresh", 10)
                .await
        }));
    }
    for task in tasks {
        task.await.unwrap().unwrap();
    }
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    let stored = store.require("relay_account_1").unwrap();
    assert_eq!(stored.access_token(), "new-access-secret");
    assert_eq!(stored.refresh_token(), Some("new-refresh-secret"));
    assert_eq!(stored.generation(), 8);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stale_lock_is_recovered_and_owner_cleanup_is_safe() {
    let root = temp_root("stale");
    let locks = ProcessAccountLocks::with_config(root.clone(), fast_lock_config()).unwrap();
    let lock_dir = root.join("locks");
    ensure_lock_dir(&lock_dir).unwrap();
    let path = lock_path(&lock_dir, "relay_account_1");
    let stale = LockOwner {
        owner_token: Uuid::new_v4().hyphenated().to_string(),
        created_at_ms: now_ms().saturating_sub(10_000),
        process_id: 999_999,
    };
    fs::write(&path, serde_json::to_vec(&stale).unwrap()).unwrap();

    let guard = locks.acquire("relay_account_1").await.unwrap();
    assert!(path.exists());
    drop(guard);
    assert!(!path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn nonblocking_lock_acquire_leaves_a_live_refresh_owner_undisturbed() {
    let root = temp_root("try-acquire");
    let locks = ProcessAccountLocks::with_config(root.clone(), fast_lock_config()).unwrap();
    let guard = locks.acquire("relay_account_1").await.unwrap();
    let started = Instant::now();

    assert!(locks.try_acquire("relay_account_1").unwrap().is_none());
    assert!(started.elapsed() < Duration::from_secs(1));

    drop(guard);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_failure_stays_pending_and_retries_without_second_refresh() {
    let root = temp_root("persistence");
    let backend = Arc::new(MemorySecrets::default());
    let store = CredentialStore::new(backend);
    let initial = expired_credentials();
    let initial_tokens = initial.to_token_set().unwrap();
    store.save(&initial).unwrap();
    let client = Arc::new(RefreshOnce {
        calls: AtomicUsize::new(0),
    });
    let refresh = StoredRefreshAdapter::with_lock_config(
        root.clone(),
        store.clone(),
        client.clone(),
        0,
        fast_lock_config(),
    )
    .unwrap();
    let metadata = Arc::new(CaptureMetadata::default());
    metadata.fail_generation.store(true, Ordering::SeqCst);
    let persistence = CredentialPersistence::new(store, metadata.clone(), root.clone());
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register("relay_account_1", initial_tokens, AccountAuthState::Active)
        .await
        .unwrap();

    assert!(matches!(
        authority
            .prepare_and_persist("relay_account_1", 10, 0, &refresh, &persistence)
            .await,
        Err(TokenAuthorityError::PersistenceFailed(_))
    ));
    metadata.fail_generation.store(false, Ordering::SeqCst);
    let prepared = authority
        .prepare_and_persist("relay_account_1", 11, 0, &refresh, &persistence)
        .await
        .unwrap();
    assert_eq!(prepared.status, PrepareStatus::Ready);
    assert_eq!(client.calls.load(Ordering::SeqCst), 1);
    assert_eq!(metadata.generation_calls.load(Ordering::SeqCst), 2);
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn removed_account_refresh_cannot_overwrite_a_readded_credential() {
    struct PausedRefresh {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
    }

    impl CodexRefreshClient for PausedRefresh {
        fn refresh<'a>(
            &'a self,
            _local_account_id: &'a str,
            _provider_account_id: Option<&'a str>,
            _refresh_token: &'a str,
            now_ms: u64,
            _kind: super::super::oauth::OAuthClientKind,
        ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
        {
            Box::pin(async move {
                self.entered.notify_one();
                self.release.notified().await;
                Ok(CredentialRefresh::new(
                    "old-refresh-access".into(),
                    Some("old-refresh-rotated".into()),
                    None,
                    Some(now_ms + 60_000),
                )
                .unwrap())
            })
        }
    }

    let root = temp_root("removed-slot-refresh");
    let store = CredentialStore::new(Arc::new(MemorySecrets::default()));
    let expired_account = expired_credentials();
    store.save(&expired_account).unwrap();
    let client = Arc::new(PausedRefresh {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
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
    let persistence = Arc::new(CredentialPersistence::new(
        store.clone(),
        Arc::new(CaptureMetadata::default()),
        root.clone(),
    ));
    let authority = Arc::new(TokenAuthority::new(1).unwrap());
    authority
        .register(
            "relay_account_1",
            expired_account.to_token_set().unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let pending = {
        let authority = authority.clone();
        let adapter = adapter.clone();
        let persistence = persistence.clone();
        tokio::spawn(async move {
            authority
                .prepare_and_persist(
                    "relay_account_1",
                    10,
                    0,
                    adapter.as_ref(),
                    persistence.as_ref(),
                )
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), client.entered.notified())
        .await
        .unwrap();
    let replacement = readd_replacement(&authority, &store).await;
    client.release.notify_one();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap()
            .unwrap(),
        Err(TokenAuthorityError::AccountNotFound)
    ));
    assert!(store
        .require("relay_account_1")
        .unwrap()
        .matches_snapshot(&replacement));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn pending_token_persistence_cannot_write_into_a_readded_account() {
    use futures_util::poll;
    use std::task::Poll;

    let root = temp_root("removed-slot-persistence");
    let store = CredentialStore::new(Arc::new(MemorySecrets::default()));
    let original = expired_credentials();
    store.save(&original).unwrap();
    let authority = TokenAuthority::new(1).unwrap();
    authority
        .register(
            "relay_account_1",
            original.to_token_set().unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    let persistence = CredentialPersistence::new(
        store.clone(),
        Arc::new(CaptureMetadata::default()),
        root.clone(),
    );
    let locks = ProcessAccountLocks::with_config(root.clone(), fast_lock_config()).unwrap();
    let held = locks.acquire("relay_account_1").await.unwrap();
    let mut pending =
        Box::pin(authority.invalidate_access_and_persist("relay_account_1", 10, &persistence));
    assert!(matches!(poll!(pending.as_mut()), Poll::Pending));

    let replacement = readd_replacement(&authority, &store).await;
    drop(held);
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(5), pending)
            .await
            .unwrap(),
        Err(TokenAuthorityError::PersistenceFailed(_))
    ));
    assert!(store
        .require("relay_account_1")
        .unwrap()
        .matches_snapshot(&replacement));
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn stale_agent_task_result_cannot_update_a_replaced_identity_with_no_task() {
    const TEST_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let root = temp_root("replaced-agent-task");
    let store = CredentialStore::new(Arc::new(MemorySecrets::default()));
    let previous_identity =
        AgentIdentityCredential::unregistered(TEST_KEY.into(), "old-runtime".into()).unwrap();
    let replacement = StoredCodexCredentials::new_agent_identity(
        "relay_account_1",
        AgentIdentityCredential::unregistered(TEST_KEY.into(), "new-runtime".into()).unwrap(),
        2,
        2,
        None,
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap();
    store.save(&replacement).unwrap();
    let persistence = CredentialPersistence::new(
        store.clone(),
        Arc::new(CaptureMetadata::default()),
        root.clone(),
    );
    assert!(persistence
        .persist_agent_task_id_for_identity("relay_account_1", &previous_identity, "old-task")
        .await
        .is_err());
    assert!(store
        .require("relay_account_1")
        .unwrap()
        .matches_snapshot(&replacement));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn refresh_and_lock_debug_output_is_redacted() {
    let refresh = CredentialRefresh::new(
        "private-access".into(),
        Some("private-refresh".into()),
        Some("private-id".into()),
        Some(1),
    )
    .unwrap();
    let rendered = format!("{refresh:?}");
    assert!(!rendered.contains("private-access"));
    assert!(!rendered.contains("private-refresh"));
    assert!(!rendered.contains("private-id"));
}

fn expired_credentials() -> StoredCodexCredentials {
    StoredCodexCredentials::new(
        "relay_account_1",
        "old-access-secret".into(),
        Some("old-refresh-secret".into()),
        Some("old-id-secret".into()),
        Some(1),
        0,
        7,
        Some("private@example.test".into()),
        Some("provider-private-id".into()),
        Some("provider-user-id".into()),
        Some("provider-org-id".into()),
        Some("plus".into()),
        false,
    )
    .unwrap()
}

async fn readd_replacement(
    authority: &TokenAuthority,
    store: &CredentialStore<MemorySecrets>,
) -> StoredCodexCredentials {
    assert!(authority.remove("relay_account_1"));
    store.delete("relay_account_1").unwrap();
    let replacement = StoredCodexCredentials::new(
        "relay_account_1",
        "replacement-access".into(),
        Some("replacement-refresh".into()),
        None,
        Some(1),
        11,
        1,
        None,
        Some("replacement-provider".into()),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    store.save(&replacement).unwrap();
    authority
        .register(
            "relay_account_1",
            replacement.to_token_set().unwrap(),
            AccountAuthState::Active,
        )
        .await
        .unwrap();
    replacement
}

fn fast_lock_config() -> ProcessLockConfig {
    ProcessLockConfig {
        wait_timeout_ms: 2_000,
        poll_interval_ms: 5,
        stale_after_ms: 5_000,
    }
}

fn temp_root(label: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("zenith-relay-authority-{label}-{}", Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    root
}
