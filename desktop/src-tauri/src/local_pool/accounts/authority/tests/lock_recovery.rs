use super::*;
use std::time::SystemTime;

fn write_lock_fixture(path: &std::path::Path, bytes: &[u8], stale: bool) {
    fs::write(path, bytes).unwrap();
    if stale {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(600)),
            )
            .unwrap();
    }
}

#[tokio::test]
async fn stale_corrupted_credential_lock_is_recovered_before_persistence() {
    for bytes in [vec![0; 99], vec![], b"{\"ownerToken\":".to_vec()] {
        let root = temp_root("corrupted-persistence-lock");
        let store = CredentialStore::new(Arc::new(MemorySecrets::default()));
        let initial = expired_credentials();
        store.save(&initial).unwrap();
        let metadata = Arc::new(CaptureMetadata::default());
        let locks =
            Arc::new(ProcessAccountLocks::with_config(root.clone(), fast_lock_config()).unwrap());
        let persistence = CredentialPersistence::new(store.clone(), metadata.clone(), locks);
        let lock_dir = root.join("locks");
        ensure_lock_dir(&lock_dir).unwrap();
        let path = lock_path(&lock_dir, "relay_account_1");
        write_lock_fixture(&path, &bytes, true);
        let updated = initial
            .apply_refresh(
                CredentialRefresh::new(
                    "rotated-access".into(),
                    Some("rotated-refresh".into()),
                    None,
                    Some(60_000),
                )
                .unwrap(),
                10,
            )
            .unwrap();
        persistence
            .persist("relay_account_1", &updated.to_token_set().unwrap())
            .await
            .unwrap();
        assert_eq!(store.require("relay_account_1").unwrap().generation(), 8);
        assert_eq!(metadata.generation_calls.load(Ordering::SeqCst), 1);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn fresh_corrupted_credential_lock_is_not_stolen() {
    let root = temp_root("fresh-corrupted-lock");
    let locks = ProcessAccountLocks::with_config(
        root.clone(),
        ProcessLockConfig {
            wait_timeout_ms: 20,
            poll_interval_ms: 5,
            stale_after_ms: 60_000,
        },
    )
    .unwrap();
    let lock_dir = root.join("locks");
    ensure_lock_dir(&lock_dir).unwrap();
    let path = lock_path(&lock_dir, "relay_account_1");
    for bytes in [vec![], vec![0; 99], b"{\"ownerToken\":".to_vec()] {
        write_lock_fixture(&path, &bytes, false);
        assert!(locks.try_acquire("relay_account_1").unwrap().is_none());
        assert_eq!(
            locks.acquire("relay_account_1").await.unwrap_err(),
            ProcessLockError::Timeout
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn nonblocking_acquire_recovers_stale_corrupted_credential_lock() {
    let root = temp_root("try-corrupted-lock");
    let locks = ProcessAccountLocks::with_config(root.clone(), fast_lock_config()).unwrap();
    let lock_dir = root.join("locks");
    ensure_lock_dir(&lock_dir).unwrap();
    let path = lock_path(&lock_dir, "relay_account_1");
    write_lock_fixture(&path, &[0; 99], true);
    let guard = locks.try_acquire("relay_account_1").unwrap().unwrap();
    assert!(locks.try_acquire("relay_account_1").unwrap().is_none());
    drop(guard);
    assert!(!path.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupted_lock_recovery_rejects_unsafe_nodes_and_future_timestamps() {
    let root = temp_root("unsafe-corrupted-lock");
    let locks = ProcessAccountLocks::with_config(root.clone(), fast_lock_config()).unwrap();
    let lock_dir = root.join("locks");
    ensure_lock_dir(&lock_dir).unwrap();
    let path = lock_path(&lock_dir, "relay_account_1");
    fs::create_dir(&path).unwrap();
    assert_eq!(
        locks.try_acquire("relay_account_1").unwrap_err(),
        ProcessLockError::UnsafePath
    );
    assert!(path.is_dir());
    fs::remove_dir(&path).unwrap();

    let oversized = vec![0; 4097];
    write_lock_fixture(&path, &oversized, true);
    assert_eq!(
        locks.try_acquire("relay_account_1").unwrap_err(),
        ProcessLockError::UnsafePath
    );
    assert_eq!(fs::read(&path).unwrap(), oversized);

    write_lock_fixture(&path, &[0; 99], false);
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(fs::FileTimes::new().set_modified(SystemTime::now() + Duration::from_secs(600)))
        .unwrap();
    assert!(locks.try_acquire("relay_account_1").unwrap().is_none());
    assert_eq!(fs::read(&path).unwrap(), vec![0; 99]);
    fs::remove_dir_all(root).unwrap();
}
