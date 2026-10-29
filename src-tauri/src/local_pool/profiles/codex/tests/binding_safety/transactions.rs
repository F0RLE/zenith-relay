use super::*;

pub(super) fn assert_unsafe_transaction_reads(path: &Path) {
    let expected = Some(b"managed".to_vec());
    let previous = Some(b"previous".to_vec());
    assert_unsafe_backup_error(read_optional_bytes(path).unwrap_err());
    assert_unsafe_backup_error(replace_if_unchanged(path, &expected, "next").unwrap_err());
    assert_unsafe_backup_error(remove_if_unchanged(path, &expected).unwrap_err());
    assert_unsafe_backup_error(rollback_file(path, "managed", &previous).unwrap_err());
    assert_unsafe_backup_error(
        restore_snapshot_if_unchanged(path, &expected, &previous).unwrap_err(),
    );
    assert_unsafe_backup_error(replace_with_snapshot(path, &expected, None).unwrap_err());
}

#[test]
fn account_backup_transaction_reads_reject_oversized_and_nonregular_nodes() {
    for directory in [false, true] {
        let (root, home, backups) = profile_dirs("backup-transaction-node");
        fs::create_dir(&backups).unwrap();
        let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
        if directory {
            fs::create_dir(&path).unwrap();
        } else {
            fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        }
        assert_unsafe_transaction_reads(&path);
        assert_eq!(path.is_dir(), directory);
        if !directory {
            assert_eq!(fs::metadata(path).unwrap().len(), 1024 * 1024 + 1);
        }
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn account_backup_transaction_reads_do_not_follow_a_leaf_symlink() {
    let (root, home, backups) = profile_dirs("backup-transaction-symlink");
    fs::create_dir(&backups).unwrap();
    let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    let foreign = root.join("foreign.json");
    fs::write(&foreign, b"managed").unwrap();
    std::os::unix::fs::symlink(&foreign, &path).unwrap();
    assert_unsafe_transaction_reads(&path);
    assert_eq!(fs::read(&foreign).unwrap(), b"managed");
    assert!(fs::symlink_metadata(path).unwrap().file_type().is_symlink());
    fs::remove_dir_all(root).unwrap();
}

pub(super) fn assert_switch_rollback_rejects_unsafe_backup(
    home: &Path,
    path: &Path,
    make_unsafe: impl FnOnce(&Path),
) {
    let _profile_guard = lock_codex_profile();
    fs::write(path, b"before").unwrap();
    let memory = MemorySecrets::default();
    memory
        .save("synthetic-transaction-secret", "before")
        .unwrap();
    let error = switch_transaction::run(home, &memory, |secrets| {
        replace_if_unchanged(path, &Some(b"before".to_vec()), "managed")?;
        secrets.save("synthetic-transaction-secret", "managed")?;
        make_unsafe(path);
        Err::<(), _>(LocalPoolError::invalid_state("synthetic failure"))
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert!(error.message.contains("file type or size is unsafe"));
    assert_eq!(
        memory
            .load("synthetic-transaction-secret")
            .unwrap()
            .as_deref(),
        Some("managed")
    );
}

#[test]
fn switch_rollback_rejects_an_oversized_backup_before_restoring_secrets() {
    let (root, home, backups) = profile_dirs("backup-journal-bound");
    fs::create_dir(&backups).unwrap();
    let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    assert_switch_rollback_rejects_unsafe_backup(&home, &path, |path| {
        fs::write(path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    });
    assert_eq!(fs::metadata(path).unwrap().len(), 1024 * 1024 + 1);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_backup_transactions_still_restore_regular_and_missing_snapshots() {
    let (root, home, backups) = profile_dirs("backup-transaction-valid");
    fs::create_dir(&backups).unwrap();
    let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    replace_if_unchanged(&path, &None, "managed").unwrap();
    rollback_file(&path, "managed", &None).unwrap();
    assert!(!path.exists());
    fs::write(&path, vec![b'x'; 1024 * 1024]).unwrap();
    assert_eq!(
        read_optional_bytes(&path).unwrap().unwrap().len(),
        1024 * 1024
    );
    fs::write(&path, b"before").unwrap();
    let _profile_guard = lock_codex_profile();
    let result = switch_transaction::run(&home, &MemorySecrets::default(), |_| {
        replace_if_unchanged(&path, &Some(b"before".to_vec()), "managed")?;
        Err::<(), _>(LocalPoolError::invalid_state("synthetic failure"))
    });
    assert!(result.is_err());
    assert_eq!(fs::read(&path).unwrap(), b"before");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn generic_config_and_auth_reads_keep_symlink_and_size_semantics() {
    let (root, home, _) = profile_dirs("backup-generic-read-control");
    let content = vec![b'x'; 1024 * 1024 + 1];
    let foreign = root.join("user-selected.json");
    fs::write(&foreign, &content).unwrap();
    for name in [CONFIG_FILE, AUTH_FILE] {
        let path = home.join(name);
        std::os::unix::fs::symlink(&foreign, &path).unwrap();
        assert_eq!(
            read_optional_bytes(&path).unwrap().as_deref(),
            Some(content.as_slice())
        );
    }
    assert_eq!(fs::read(foreign).unwrap(), content);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn generic_config_and_auth_regular_reads_keep_their_existing_size_semantics() {
    let (root, home, _) = profile_dirs("backup-generic-size-control");
    let content = vec![b'x'; 1024 * 1024 + 1];
    for name in [CONFIG_FILE, AUTH_FILE] {
        let path = home.join(name);
        fs::write(&path, &content).unwrap();
        assert_eq!(read_optional_bytes(&path).unwrap(), Some(content.clone()));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_backup_case_aliases_keep_the_strict_read_policy() {
    let (root, home, backups) = profile_dirs("backup-case-alias");
    fs::create_dir(&backups).unwrap();
    let original = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    let path = original.with_file_name(
        original
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_ascii_uppercase(),
    );
    fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
    assert_unsafe_transaction_reads(&path);
    fs::remove_dir_all(root).unwrap();
}
