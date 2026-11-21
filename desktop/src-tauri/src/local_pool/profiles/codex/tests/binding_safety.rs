use super::*;

mod direct_reads;
mod transactions;

fn assert_unsafe_backup_error(error: LocalPoolError) {
    assert_eq!(error.code, ErrorCode::Io);
    assert!(error.message.contains("file type or size is unsafe"));
}

fn account_backup_document(home: &Path) -> serde_json::Value {
    serde_json::json!({
        "version": 1,
        "profileDir": canonical_profile_dir(home).unwrap().to_string_lossy(),
        "previousModelProvider": null,
        "previousAuthSecretRef": null,
        "managedAccountId": "synthetic-account",
        "managedAccessHash": "0".repeat(64),
    })
}

#[test]
fn account_backup_inventory_rejects_mismatched_profile_identity() {
    let (root, home, backups) = profile_dirs("backup-identity");
    fs::create_dir(&backups).unwrap();
    let expected = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    fs::write(&expected, account_backup_document(&home).to_string()).unwrap();
    assert_eq!(account_bindings(&backups).unwrap().len(), 1);
    fs::rename(&expected, backups.join("codex-account-other.json")).unwrap();
    assert!(account_bindings(&backups).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_backup_inventory_and_token_refresh_reject_oversized_metadata() {
    let (root, home, backups) = profile_dirs("backup-bounds");
    fs::create_dir(&backups).unwrap();
    let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    let mut document = account_backup_document(&home);
    document["padding"] = serde_json::json!("x".repeat(1024 * 1024));
    fs::write(path, document.to_string()).unwrap();
    let tokens = TokenSet::new("synthetic-access", None, None, None, 1, 1).unwrap();
    assert!(account_bindings(&backups).is_err());
    assert!(
        managed_account_token_update(
            &home,
            &backups,
            "synthetic-account",
            &tokens,
            "synthetic-provider-account",
        )
        .is_err()
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn account_backup_inventory_does_not_follow_a_leaf_symlink() {
    let (root, home, backups) = profile_dirs("backup-symlink");
    fs::create_dir(&backups).unwrap();
    let foreign = root.join("foreign.json");
    let original = account_backup_document(&home).to_string();
    fs::write(&foreign, &original).unwrap();
    let path = account_backup_path(&backups, &canonical_profile_dir(&home).unwrap());
    std::os::unix::fs::symlink(&foreign, &path).unwrap();
    assert!(account_bindings(&backups).is_err());
    assert_eq!(fs::read_to_string(&foreign).unwrap(), original);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_backup_writer_cannot_publish_metadata_beyond_the_read_limit() {
    let (root, home, _) = profile_dirs("backup-write-bound");
    let mut document = account_backup_document(&home);
    document["previousModelProvider"] = serde_json::json!("x".repeat(1024 * 1024));
    let backup: AccountProfileBackup = serde_json::from_value(document).unwrap();
    assert!(serialize_account_backup(&backup).is_err());
    fs::remove_dir_all(root).unwrap();
}
