use super::*;

pub(super) fn assert_direct_reads_reject(home: &Path, backups: &Path) {
    let secrets = MemorySecrets::default();
    let tokens = TokenSet::new("synthetic-access", None, None, None, 1, 1).unwrap();
    assert_unsafe_backup_error(active_managed_account_id(home, backups).unwrap_err());
    assert_unsafe_backup_error(
        account_ops::snapshot_user_profile_with(home, backups, &secrets)
            .err()
            .expect("unsafe account backup must be rejected before snapshotting"),
    );
    assert_unsafe_backup_error(
        attach_account_with(
            home,
            backups,
            "synthetic-account",
            &tokens,
            "provider",
            &secrets,
        )
        .unwrap_err(),
    );
    assert_unsafe_backup_error(restore_account_with(home, backups, &secrets).unwrap_err());
    assert_unsafe_backup_error(
        account::sync_account_profile_with(home, backups, &tokens, "provider").unwrap_err(),
    );
    assert_unsafe_backup_error(
        switch_to_account_with_intent(
            home,
            backups,
            "synthetic-account",
            &tokens,
            "provider",
            true,
            &secrets,
        )
        .unwrap_err(),
    );
    assert_unsafe_backup_error(
        switch_to_local_with(
            home,
            backups,
            "synthetic-key",
            "http://127.0.0.1:14998/v1",
            "synthetic-secret",
            LocalAttachOptions::default(),
            &secrets,
        )
        .unwrap_err(),
    );
}

fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let (root, home, backups) = profile_dirs(name);
    fs::create_dir(&backups).unwrap();
    fs::write(home.join(CONFIG_FILE), "model_provider = 'external'\n").unwrap();
    let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    (root, home, backups, path)
}

#[test]
fn direct_account_backup_reads_reject_oversized_and_nonregular_nodes() {
    for directory in [false, true] {
        let (root, home, backups, path) = fixture("backup-direct-node");
        if directory {
            fs::create_dir(&path).unwrap();
        } else {
            fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        }
        let config = fs::read(home.join(CONFIG_FILE)).unwrap();
        assert_direct_reads_reject(&home, &backups);
        assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config);
        assert!(!home.join(AUTH_FILE).exists());
        assert_eq!(path.is_dir(), directory);
        if !directory {
            assert_eq!(fs::metadata(&path).unwrap().len(), 1024 * 1024 + 1);
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn direct_account_backup_reads_preserve_missing_and_regular_file_semantics() {
    let (root, home, backups, path) = fixture("backup-direct-optional");
    assert!(read_optional_bytes(&path).unwrap().is_none());
    assert!(
        active_managed_account_id(&home, &backups)
            .unwrap()
            .is_none()
    );
    assert!(
        restore_account_with(&home, &backups, &MemorySecrets::default())
            .unwrap()
            .is_none()
    );
    let original = account_backup_document(&home).to_string();
    fs::write(&path, &original).unwrap();
    assert_eq!(
        read_optional_bytes(&path).unwrap(),
        Some(original.into_bytes())
    );
    assert_eq!(
        active_managed_account_id(&home, &backups)
            .unwrap()
            .as_deref(),
        Some("synthetic-account")
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn direct_account_backup_reads_reject_existing_and_dangling_leaf_symlinks() {
    for dangling in [false, true] {
        let (root, home, backups, path) = fixture("backup-direct-symlink");
        let foreign = root.join("foreign.json");
        let original = account_backup_document(&home).to_string();
        if !dangling {
            fs::write(&foreign, &original).unwrap();
        }
        std::os::unix::fs::symlink(&foreign, &path).unwrap();
        assert_direct_reads_reject(&home, &backups);
        assert!(
            fs::symlink_metadata(&path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        if !dangling {
            assert_eq!(fs::read_to_string(&foreign).unwrap(), original);
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn restore_rechecks_account_backup_with_the_same_pre_read_bound() {
    let (root, home, backups) = profile_dirs("backup-restore-reread");
    fs::write(home.join(CONFIG_FILE), "model_provider = 'openai'\n").unwrap();
    let memory = MemorySecrets::default();
    let tokens = TokenSet::new("synthetic-access", None, None, None, 1, 1).unwrap();
    attach_account_with(
        &home,
        &backups,
        "synthetic-account",
        &tokens,
        "provider",
        &memory,
    )
    .unwrap();
    let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    let secrets = MutatingLoadSecrets::new(path.clone(), vec![b'x'; 1024 * 1024 + 1]);
    *secrets.values.lock().unwrap() = memory.0.lock().unwrap().clone();
    let config = fs::read(home.join(CONFIG_FILE)).unwrap();
    let auth = fs::read(home.join(AUTH_FILE)).unwrap();

    assert_unsafe_backup_error(restore_account_with(&home, &backups, &secrets).unwrap_err());
    assert_eq!(fs::read(home.join(CONFIG_FILE)).unwrap(), config);
    assert_eq!(fs::read(home.join(AUTH_FILE)).unwrap(), auth);
    assert_eq!(fs::metadata(path).unwrap().len(), 1024 * 1024 + 1);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn account_backup_fifo_paths_do_not_block_direct_or_transactional_reads() {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    if let Some(root) = fifo_child_root() {
        let home = root.join("profile");
        let backups = root.join("backups");
        fs::create_dir(&home).unwrap();
        fs::create_dir(&backups).unwrap();
        fs::write(home.join(CONFIG_FILE), "model_provider = 'external'\n").unwrap();
        let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
        // The parent exclusively reserved the containing fixture before spawning.
        let create_fifo = |path: &Path| {
            assert!(
                Command::new("mkfifo")
                    .args(["-m", "600"])
                    .arg(path)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        create_fifo(&path);
        assert_direct_reads_reject(&home, &backups);
        super::transactions::assert_unsafe_transaction_reads(&path);
        fs::remove_file(&path).unwrap();
        super::transactions::assert_switch_rollback_rejects_unsafe_backup(&home, &path, |path| {
            fs::remove_file(path).unwrap();
            create_fifo(path);
        });
        fs::write(root.join("completed"), b"checked").unwrap();
        return;
    }

    let root = crate::files::test_temp_root("profile-backup-fifo");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "account_backup_fifo_paths_do_not_block_direct_or_transactional_reads",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(FIFO_TEST_ROOT_ENV, &root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let kill = child.kill();
            let wait = child.wait();
            let cleanup = fs::remove_dir_all(&root);
            panic!(
                "account backup FIFO read timed out; kill={kill:?}; wait={wait:?}; cleanup={cleanup:?}"
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let completed = root.join("completed").is_file();
    fs::remove_dir_all(root).unwrap();
    assert!(
        status.success() && completed,
        "the isolated FIFO checks must execute and pass"
    );
}

#[cfg(unix)]
const FIFO_TEST_ROOT_ENV: &str = "ZENITH_RELAY_ACCOUNT_BACKUP_FIFO_TEST_ROOT";

#[cfg(unix)]
fn fifo_child_root() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let root = PathBuf::from(std::env::var_os(FIFO_TEST_ROOT_ENV)?);
    // This is a parent-to-current_exe test channel, not an application path
    // selector. Only accept the private fixture reserved by this test's parent.
    let temporary_directory = std::env::temp_dir();
    assert_eq!(root.parent(), Some(temporary_directory.as_path()));
    let name = root.file_name().and_then(|name| name.to_str()).unwrap();
    let nonce = name
        .strip_prefix("zenith-relay-profile-backup-fifo-")
        .expect("the parent must pass this test's reserved fixture");
    assert!(nonce.len() == 32 && nonce.bytes().all(|byte| byte.is_ascii_hexdigit()));
    let metadata = fs::symlink_metadata(&root).unwrap();
    assert!(metadata.is_dir() && !metadata.file_type().is_symlink());
    assert_eq!(metadata.permissions().mode() & 0o777, 0o700);
    Some(root)
}
