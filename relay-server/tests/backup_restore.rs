use base64::{engine::general_purpose::STANDARD, Engine};
use std::{fs, path::Path, process::Command, sync::Arc};
use tempfile::TempDir;
use url::Url;
use zenith_relay_server::{
    config::Config,
    state::AppState,
    store::{Store, Vault},
};

#[test]
fn cli_backup_and_restore_preserve_database_and_encrypted_vault() {
    let root = TempDir::new().unwrap();
    let source = root.path().join("source");
    let restored = root.path().join("restored");
    let backup = root.path().join("backup");
    fs::create_dir_all(&source).unwrap();
    let store = Arc::new(Store::open(source.join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&source.join("vault"), [7; 32]).unwrap());
    let state = AppState::new(
        Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: source.clone(),
            public_base_url: Url::parse("http://127.0.0.1:14999").unwrap(),
            management_token: "synthetic-management-token-value".into(),
            vault_key: [7; 32],
        },
        store,
        vault,
    )
    .unwrap();
    state
        .vault
        .save("account:synthetic", "synthetic-encrypted-secret")
        .unwrap();
    let server_id = state.store.server_id().unwrap();
    let system_key = state
        .store
        .keys()
        .unwrap()
        .into_iter()
        .find(|key| key.system)
        .unwrap();
    let system_secret = state.vault.load(&system_key.secret_ref).unwrap().unwrap();
    drop(state);

    run_cli(&source, "--backup", &backup);
    let manifest = fs::read_to_string(backup.join("manifest.json")).unwrap();
    assert!(manifest.contains("zenith-relay-server-backup"));
    assert!(!manifest.contains("synthetic-encrypted-secret"));

    run_cli(&restored, "--restore", &backup);
    let restored_store = Store::open(restored.join("relay.sqlite")).unwrap();
    assert_eq!(restored_store.server_id().unwrap(), server_id);
    assert_eq!(
        restored_store
            .keys()
            .unwrap()
            .into_iter()
            .find(|key| key.system)
            .unwrap()
            .id,
        system_key.id
    );
    let restored_vault = Vault::open(&restored.join("vault"), [7; 32]).unwrap();
    assert_eq!(
        restored_vault.load("account:synthetic").unwrap().as_deref(),
        Some("synthetic-encrypted-secret")
    );
    assert_eq!(
        restored_vault
            .load(&system_key.secret_ref)
            .unwrap()
            .as_deref(),
        Some(system_secret.as_str())
    );
}

#[test]
fn invalid_vault_key_does_not_replace_a_valid_live_store() {
    let root = TempDir::new().unwrap();
    let source = root.path().join("source");
    let live = root.path().join("live");
    let backup = root.path().join("backup");
    fs::create_dir_all(&source).unwrap();
    let source_store = Store::open(source.join("relay.sqlite")).unwrap();
    let source_id = source_store.server_id().unwrap();
    let source_vault = Vault::open(&source.join("vault"), [7; 32]).unwrap();
    source_vault
        .save("account:synthetic", "source-secret")
        .unwrap();
    drop(source_vault);
    drop(source_store);
    run_cli(&source, "--backup", &backup);

    fs::create_dir_all(&live).unwrap();
    let live_store = Store::open(live.join("relay.sqlite")).unwrap();
    let live_id = live_store.server_id().unwrap();
    assert_ne!(source_id, live_id);
    let live_vault = Vault::open(&live.join("vault"), [8; 32]).unwrap();
    live_vault.save("account:synthetic", "live-secret").unwrap();
    drop(live_vault);
    drop(live_store);

    let output = command(&live, "--restore", &backup, [8; 32])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("vault decryption failed"));

    let reopened_store = Store::open(live.join("relay.sqlite")).unwrap();
    assert_eq!(reopened_store.server_id().unwrap(), live_id);
    let reopened_vault = Vault::open(&live.join("vault"), [8; 32]).unwrap();
    assert_eq!(
        reopened_vault.load("account:synthetic").unwrap().as_deref(),
        Some("live-secret")
    );
}

fn run_cli(data_dir: &Path, operation: &str, path: &Path) {
    let status = command(data_dir, operation, path, [7; 32])
        .status()
        .unwrap();
    assert!(status.success(), "{operation} failed");
}

fn command(data_dir: &Path, operation: &str, path: &Path, vault_key: [u8; 32]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_zenith-relay-server"));
    command
        .arg(operation)
        .arg(path)
        .env("ZENITH_RELAY_DATA_DIR", data_dir)
        .env(
            "ZENITH_RELAY_MANAGEMENT_TOKEN",
            "synthetic-management-token-value",
        )
        .env("ZENITH_RELAY_VAULT_KEY", STANDARD.encode(vault_key));
    command
}
