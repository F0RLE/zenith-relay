use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;
use zenith_relay_server::store::{Store, Vault};

#[test]
fn cli_backup_and_restore_preserve_database_and_encrypted_vault() {
    let root = TempDir::new().unwrap();
    let source = root.path().join("source");
    let restored = root.path().join("restored");
    let backup = root.path().join("backup");
    fs::create_dir_all(&source).unwrap();
    Store::open(source.join("relay.sqlite")).unwrap();
    let vault = Vault::open(&source.join("vault"), [7; 32]).unwrap();
    vault
        .save("account:synthetic", "synthetic-encrypted-secret")
        .unwrap();

    run_cli(&source, "--backup", &backup);
    let manifest = fs::read_to_string(backup.join("manifest.json")).unwrap();
    assert!(manifest.contains("zenith-relay-server-backup"));
    assert!(!manifest.contains("synthetic-encrypted-secret"));

    run_cli(&restored, "--restore", &backup);
    Store::open(restored.join("relay.sqlite")).unwrap();
    let restored_vault = Vault::open(&restored.join("vault"), [7; 32]).unwrap();
    assert_eq!(
        restored_vault.load("account:synthetic").unwrap().as_deref(),
        Some("synthetic-encrypted-secret")
    );
}

fn run_cli(data_dir: &Path, operation: &str, path: &Path) {
    let status = Command::new(env!("CARGO_BIN_EXE_zenith-relay-server"))
        .arg(operation)
        .arg(path)
        .env("ZENITH_RELAY_DATA_DIR", data_dir)
        .env(
            "ZENITH_RELAY_MANAGEMENT_TOKEN",
            "synthetic-management-token-value",
        )
        .env("ZENITH_RELAY_VAULT_KEY", STANDARD.encode([7_u8; 32]))
        .status()
        .unwrap();
    assert!(status.success());
}
