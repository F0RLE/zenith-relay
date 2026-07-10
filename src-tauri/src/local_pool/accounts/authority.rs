use super::{
    credentials::{CredentialRefresh, CredentialStore},
    import_session::SecretBackend,
    oauth::CodexOAuthClient,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::time::{sleep, Duration, Instant};
use uuid::Uuid;
use zenith_relay_core::accounts::{
    AccountAuthState, TokenPersistenceAdapter, TokenPersistenceFailure, TokenRefresh,
    TokenRefreshAdapter, TokenRefreshFailure, TokenRefreshFailureKind, TokenSet,
};

const MAX_LOCK_BYTES: u64 = 4 * 1024;

pub trait CodexRefreshClient: Send + Sync {
    fn refresh<'a>(
        &'a self,
        provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>;
}

impl CodexRefreshClient for CodexOAuthClient {
    fn refresh<'a>(
        &'a self,
        _provider_account_id: Option<&'a str>,
        refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<CredentialRefresh, TokenRefreshFailure>> + Send + 'a>>
    {
        Box::pin(async move {
            let tokens = self.exchange_refresh_token(refresh_token, now_ms).await?;
            CredentialRefresh::from_oauth(tokens).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_refresh_response",
                )
            })
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcessLockConfig {
    pub wait_timeout_ms: u64,
    pub poll_interval_ms: u64,
    pub stale_after_ms: u64,
}

impl Default for ProcessLockConfig {
    fn default() -> Self {
        Self {
            wait_timeout_ms: 5_000,
            poll_interval_ms: 25,
            stale_after_ms: 120_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcessLockError {
    InvalidConfiguration,
    InvalidIdentity,
    Io,
    Timeout,
    UnsafePath,
}

#[derive(Clone)]
pub struct ProcessAccountLocks {
    root: PathBuf,
    config: ProcessLockConfig,
}

impl ProcessAccountLocks {
    pub fn with_config(root: PathBuf, config: ProcessLockConfig) -> Result<Self, ProcessLockError> {
        if config.wait_timeout_ms == 0
            || config.poll_interval_ms == 0
            || config.stale_after_ms <= config.poll_interval_ms
        {
            return Err(ProcessLockError::InvalidConfiguration);
        }
        Ok(Self { root, config })
    }

    pub async fn acquire(
        &self,
        local_account_id: &str,
    ) -> Result<ProcessAccountGuard, ProcessLockError> {
        validate_local_account_id(local_account_id)?;
        let lock_dir = self.root.join("locks");
        ensure_lock_dir(&lock_dir)?;
        let path = lock_path(&lock_dir, local_account_id);
        let deadline = Instant::now() + Duration::from_millis(self.config.wait_timeout_ms);
        loop {
            let owner = LockOwner {
                owner_token: Uuid::new_v4().hyphenated().to_string(),
                created_at_ms: now_ms(),
                process_id: std::process::id(),
            };
            match create_lock(&path, &owner) {
                Ok(()) => {
                    return Ok(ProcessAccountGuard {
                        path,
                        owner_token: owner.owner_token,
                    });
                }
                Err(CreateLockError::Exists) => {
                    let _ = recover_stale_lock(&path, self.config.stale_after_ms);
                    if Instant::now() >= deadline {
                        return Err(ProcessLockError::Timeout);
                    }
                    sleep(Duration::from_millis(self.config.poll_interval_ms)).await;
                }
                Err(CreateLockError::Unsafe) => return Err(ProcessLockError::UnsafePath),
                Err(CreateLockError::Io) => return Err(ProcessLockError::Io),
            }
        }
    }
}

pub struct ProcessAccountGuard {
    path: PathBuf,
    owner_token: String,
}

impl fmt::Debug for ProcessAccountGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessAccountGuard")
            .field("owner_token", &"[redacted]")
            .finish()
    }
}

impl Drop for ProcessAccountGuard {
    fn drop(&mut self) {
        let Ok(bytes) = fs::read(&self.path) else {
            return;
        };
        let Ok(owner) = serde_json::from_slice::<LockOwner>(&bytes) else {
            return;
        };
        if owner.owner_token == self.owner_token {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub struct StoredRefreshAdapter<B, C> {
    credentials: CredentialStore<B>,
    client: Arc<C>,
    locks: ProcessAccountLocks,
    refresh_skew_ms: u64,
}

impl<B, C> StoredRefreshAdapter<B, C> {
    pub fn new(
        root: PathBuf,
        credentials: CredentialStore<B>,
        client: Arc<C>,
        refresh_skew_ms: u64,
    ) -> Result<Self, ProcessLockError> {
        Self::with_lock_config(
            root,
            credentials,
            client,
            refresh_skew_ms,
            ProcessLockConfig::default(),
        )
    }

    pub fn with_lock_config(
        root: PathBuf,
        credentials: CredentialStore<B>,
        client: Arc<C>,
        refresh_skew_ms: u64,
        config: ProcessLockConfig,
    ) -> Result<Self, ProcessLockError> {
        Ok(Self {
            credentials,
            client,
            locks: ProcessAccountLocks::with_config(root, config)?,
            refresh_skew_ms,
        })
    }
}

impl<B, C> TokenRefreshAdapter for StoredRefreshAdapter<B, C>
where
    B: SecretBackend + Send + Sync,
    C: CodexRefreshClient,
{
    fn refresh<'a>(
        &'a self,
        local_account_id: &'a str,
        _stale_refresh_token: &'a str,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<TokenRefresh, TokenRefreshFailure>> + Send + 'a>> {
        Box::pin(async move {
            let _guard = self
                .locks
                .acquire(local_account_id)
                .await
                .map_err(lock_refresh_failure)?;
            let current = self.credentials.require(local_account_id).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "credential_load_failed",
                )
            })?;
            if current.is_access_usable(now_ms, self.refresh_skew_ms) {
                return current.to_token_refresh().map_err(|_| {
                    TokenRefreshFailure::new(
                        TokenRefreshFailureKind::Transient,
                        "invalid_stored_credential",
                    )
                });
            }
            let refresh_token = current.refresh_token().ok_or_else(|| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::ExpiredRefreshToken,
                    "refresh_token_missing",
                )
            })?;
            let refreshed = self
                .client
                .refresh(current.provider_account_id(), refresh_token, now_ms)
                .await?;
            let updated = current.apply_refresh(refreshed, now_ms).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_refresh_response",
                )
            })?;
            self.credentials.save(&updated).map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "credential_persist_failed",
                )
            })?;
            updated.to_token_refresh().map_err(|_| {
                TokenRefreshFailure::new(
                    TokenRefreshFailureKind::Transient,
                    "invalid_stored_credential",
                )
            })
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MetadataSinkError;

pub trait AccountMetadataSink: Send + Sync {
    fn persist_generation<'a>(
        &'a self,
        local_account_id: &'a str,
        generation: u64,
        updated_at_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetadataSinkError>> + Send + 'a>>;

    fn persist_auth_state<'a>(
        &'a self,
        local_account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> Pin<Box<dyn Future<Output = Result<(), MetadataSinkError>> + Send + 'a>>;
}

pub struct CredentialPersistence<B, M> {
    credentials: CredentialStore<B>,
    metadata: Arc<M>,
}

impl<B, M> CredentialPersistence<B, M> {
    pub fn new(credentials: CredentialStore<B>, metadata: Arc<M>) -> Self {
        Self {
            credentials,
            metadata,
        }
    }
}

impl<B, M> TokenPersistenceAdapter for CredentialPersistence<B, M>
where
    B: SecretBackend + Send + Sync,
    M: AccountMetadataSink,
{
    fn persist<'a>(
        &'a self,
        local_account_id: &'a str,
        tokens: &'a TokenSet,
    ) -> Pin<Box<dyn Future<Output = Result<(), TokenPersistenceFailure>> + Send + 'a>> {
        Box::pin(async move {
            let current = self
                .credentials
                .require(local_account_id)
                .map_err(|_| TokenPersistenceFailure::new("credential_load_failed"))?;
            let stored = if current.generation() > tokens.generation()
                || (current.generation() == tokens.generation()
                    && current.issued_at_ms() >= tokens.issued_at_ms())
            {
                current
            } else {
                let updated = current
                    .with_token_set(tokens)
                    .map_err(|_| TokenPersistenceFailure::new("invalid_token_set"))?;
                self.credentials
                    .save(&updated)
                    .map_err(|_| TokenPersistenceFailure::new("credential_persist_failed"))?;
                updated
            };
            self.metadata
                .persist_generation(local_account_id, stored.generation(), stored.issued_at_ms())
                .await
                .map_err(|_| TokenPersistenceFailure::new("metadata_persist_failed"))
        })
    }

    fn persist_auth_state<'a>(
        &'a self,
        local_account_id: &'a str,
        auth_state: AccountAuthState,
    ) -> Pin<Box<dyn Future<Output = Result<(), TokenPersistenceFailure>> + Send + 'a>> {
        Box::pin(async move {
            self.metadata
                .persist_auth_state(local_account_id, auth_state)
                .await
                .map_err(|_| TokenPersistenceFailure::new("metadata_persist_failed"))
        })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LockOwner {
    owner_token: String,
    created_at_ms: u64,
    process_id: u32,
}

enum CreateLockError {
    Exists,
    Io,
    Unsafe,
}

fn create_lock(path: &Path, owner: &LockOwner) -> Result<(), CreateLockError> {
    let bytes = serde_json::to_vec(owner).map_err(|_| CreateLockError::Io)?;
    if bytes.len() as u64 > MAX_LOCK_BYTES {
        return Err(CreateLockError::Io);
    }
    let mut file = match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(open_error) => {
            let retry_if_missing = matches!(
                open_error.kind(),
                std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
            );
            return match fs::symlink_metadata(path) {
                Ok(metadata)
                    if metadata.file_type().is_file() && !metadata.file_type().is_symlink() =>
                {
                    Err(CreateLockError::Exists)
                }
                Ok(_) => Err(CreateLockError::Unsafe),
                Err(_) if retry_if_missing => Err(CreateLockError::Exists),
                Err(_) => Err(CreateLockError::Io),
            };
        }
    };
    if file.write_all(&bytes).is_err() || file.sync_all().is_err() {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(CreateLockError::Io);
    }
    Ok(())
}

fn recover_stale_lock(path: &Path, stale_after_ms: u64) -> Result<bool, ProcessLockError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| ProcessLockError::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.file_type().is_file()
        || metadata.len() > MAX_LOCK_BYTES
    {
        return Err(ProcessLockError::UnsafePath);
    }
    let first = fs::read(path).map_err(|_| ProcessLockError::Io)?;
    let owner: LockOwner =
        serde_json::from_slice(&first).map_err(|_| ProcessLockError::UnsafePath)?;
    if Uuid::parse_str(&owner.owner_token).is_err()
        || now_ms().saturating_sub(owner.created_at_ms) <= stale_after_ms
    {
        return Ok(false);
    }
    let second = fs::read(path).map_err(|_| ProcessLockError::Io)?;
    if first != second {
        return Ok(false);
    }
    fs::remove_file(path).map_err(|_| ProcessLockError::Io)?;
    Ok(true)
}

fn ensure_lock_dir(path: &Path) -> Result<(), ProcessLockError> {
    fs::create_dir_all(path).map_err(|_| ProcessLockError::Io)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| ProcessLockError::Io)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(ProcessLockError::UnsafePath);
    }
    Ok(())
}

fn lock_path(lock_dir: &Path, local_account_id: &str) -> PathBuf {
    let digest = format!("{:x}", Sha256::digest(local_account_id.as_bytes()));
    lock_dir.join(format!("{}.refresh.lock", &digest[..32]))
}

fn validate_local_account_id(value: &str) -> Result<(), ProcessLockError> {
    let valid = !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'));
    if valid {
        Ok(())
    } else {
        Err(ProcessLockError::InvalidIdentity)
    }
}

fn lock_refresh_failure(error: ProcessLockError) -> TokenRefreshFailure {
    let code = match error {
        ProcessLockError::Timeout => "refresh_lock_timeout",
        ProcessLockError::InvalidIdentity => "invalid_account_id",
        ProcessLockError::InvalidConfiguration => "refresh_lock_configuration",
        ProcessLockError::Io | ProcessLockError::UnsafePath => "refresh_lock_unavailable",
    };
    TokenRefreshFailure::new(TokenRefreshFailureKind::Transient, code)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::super::{
        credentials::StoredCodexCredentials,
        import_session::{SecretBackend, SecretBackendError},
    };
    use super::*;
    use std::{
        collections::HashMap,
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            Mutex,
        },
    };
    use zenith_relay_core::accounts::{PrepareStatus, TokenAuthority, TokenAuthorityError};

    #[derive(Default)]
    struct MemorySecrets {
        values: Mutex<HashMap<String, String>>,
        fail_save: AtomicBool,
    }

    impl SecretBackend for MemorySecrets {
        fn save(&self, secret_ref: &str, value: &str) -> Result<(), SecretBackendError> {
            if self.fail_save.load(Ordering::SeqCst) {
                return Err(SecretBackendError);
            }
            self.values
                .lock()
                .unwrap()
                .insert(secret_ref.into(), value.into());
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
            provider_account_id: Option<&'a str>,
            refresh_token: &'a str,
            now_ms: u64,
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
        let persistence = CredentialPersistence::new(store, metadata.clone());
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
}
