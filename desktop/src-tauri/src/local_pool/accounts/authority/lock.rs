use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::Write,
    path::{Path, PathBuf},
};
use tokio::time::{Duration, Instant, sleep};
use uuid::Uuid;
use zenith_relay_core::accounts::TokenRefreshFailure;
pub use zenith_relay_core::providers::chatgpt::connector::authority::CredentialLockError as ProcessLockError;
use zenith_relay_core::providers::chatgpt::connector::authority::CredentialLocks;
use zenith_relay_core::unix_time_ms as now_ms;

const MAX_LOCK_BYTES: u64 = 4 * 1024;

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

    /// Attempts to acquire a credential lock without waiting for its current
    /// owner. Compensating transactions use this after an asynchronous
    /// operation has failed: if a newer login or refresh owns the lock, the
    /// old transaction must leave its state alone instead of waiting and then
    /// restoring a stale snapshot.
    pub fn try_acquire(
        &self,
        local_account_id: &str,
    ) -> Result<Option<ProcessAccountGuard>, ProcessLockError> {
        validate_local_account_id(local_account_id)?;
        let lock_dir = self.root.join("locks");
        ensure_lock_dir(&lock_dir)?;
        let path = lock_path(&lock_dir, local_account_id);
        for _ in 0..2 {
            let owner = LockOwner {
                owner_token: Uuid::new_v4().hyphenated().to_string(),
                created_at_ms: now_ms(),
                process_id: std::process::id(),
            };
            match create_lock(&path, &owner) {
                Ok(()) => {
                    return Ok(Some(ProcessAccountGuard {
                        path,
                        owner_token: owner.owner_token,
                    }));
                }
                Err(CreateLockError::Exists) => {
                    if !recover_stale_lock(&path, self.config.stale_after_ms)? {
                        return Ok(None);
                    }
                }
                Err(CreateLockError::Unsafe) => return Err(ProcessLockError::UnsafePath),
                Err(CreateLockError::Io) => return Err(ProcessLockError::Io),
            }
        }
        Ok(None)
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
        let Ok(bytes) = crate::files::read_regular_file_bounded(&self.path, MAX_LOCK_BYTES) else {
            return;
        };
        let Ok(owner) = zenith_relay_core::bounded_json::from_slice::<LockOwner>(&bytes) else {
            return;
        };
        if owner.owner_token == self.owner_token {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LockOwner {
    pub(super) owner_token: String,
    pub(super) created_at_ms: u64,
    pub(super) process_id: u32,
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
    let first = crate::files::read_regular_file_bounded(path, MAX_LOCK_BYTES)
        .map_err(|_| ProcessLockError::UnsafePath)?;
    let owner: LockOwner = zenith_relay_core::bounded_json::from_slice(&first)
        .map_err(|_| ProcessLockError::UnsafePath)?;
    if Uuid::parse_str(&owner.owner_token).is_err()
        || now_ms().saturating_sub(owner.created_at_ms) <= stale_after_ms
    {
        return Ok(false);
    }
    let second = crate::files::read_regular_file_bounded(path, MAX_LOCK_BYTES)
        .map_err(|_| ProcessLockError::UnsafePath)?;
    if first != second {
        return Ok(false);
    }
    fs::remove_file(path).map_err(|_| ProcessLockError::Io)?;
    Ok(true)
}

pub(super) fn ensure_lock_dir(path: &Path) -> Result<(), ProcessLockError> {
    fs::create_dir_all(path).map_err(|_| ProcessLockError::Io)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| ProcessLockError::Io)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(ProcessLockError::UnsafePath);
    }
    Ok(())
}

pub(super) fn lock_path(lock_dir: &Path, local_account_id: &str) -> PathBuf {
    let digest = hex::encode(Sha256::digest(local_account_id.as_bytes()));
    lock_dir.join(format!("{}.refresh.lock", &digest[..32]))
}

fn validate_local_account_id(account_id: &str) -> Result<(), ProcessLockError> {
    if zenith_relay_core::is_ascii_token(account_id, 128) {
        Ok(())
    } else {
        Err(ProcessLockError::InvalidIdentity)
    }
}

impl CredentialLocks for ProcessAccountLocks {
    fn acquire<'a>(
        &'a self,
        account_id: &'a str,
    ) -> futures_util::future::BoxFuture<'a, Result<Box<dyn Send>, ProcessLockError>> {
        Box::pin(async move {
            ProcessAccountLocks::acquire(self, account_id)
                .await
                .map(|guard| Box::new(guard) as Box<dyn Send>)
        })
    }

    fn renewal_failed(&self, account_id: &str, failure: &TokenRefreshFailure) {
        super::diagnostics::record_renewal_failure(account_id, failure);
    }
}
