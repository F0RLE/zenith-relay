use super::snapshot::{prepared_secret_ref, prepared_snapshot_path, secret_ref, snapshot_path};
use super::*;
use serde_json::Value;
use zenith_relay_core::accounts::MAX_IMPORT_ITEMS;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
};

const API_KEY: &str = "synthetic-session-api-key";
const ACCESS_TOKEN: &str = "synthetic-access-token";
const EMAIL: &str = "session.user@example.test";
const FIXED_ID: &str = "11111111-2222-4333-8444-555555555555";

#[derive(Clone, Default)]
struct MemorySecrets(Arc<Mutex<MemorySecretState>>);

#[derive(Default)]
struct MemorySecretState {
    values: HashMap<String, String>,
    fail_save: bool,
    fail_load: bool,
    fail_delete: bool,
}

impl MemorySecrets {
    fn state(&self) -> MutexGuard<'_, MemorySecretState> {
        zenith_relay_core::poison::mutex(&self.0)
    }

    fn contains(&self, secret_ref: &str) -> bool {
        self.state().values.contains_key(secret_ref)
    }
}

impl SecretBackend for MemorySecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<(), SecretBackendError> {
        let mut state = self.state();
        if state.fail_save {
            return Err(SecretBackendError);
        }
        state
            .values
            .insert(secret_ref.to_string(), secret_value.to_string());
        Ok(())
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
        let state = self.state();
        if state.fail_load {
            return Err(SecretBackendError);
        }
        Ok(state.values.get(secret_ref).cloned())
    }

    fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
        let mut state = self.state();
        if state.fail_delete {
            return Err(SecretBackendError);
        }
        state.values.remove(secret_ref);
        Ok(())
    }
}

#[derive(Clone)]
struct VaultSecrets(Arc<crate::local_pool::store::vault::Vault>);

impl SecretBackend for VaultSecrets {
    fn save(&self, secret_ref: &str, secret_value: &str) -> Result<(), SecretBackendError> {
        self.0
            .save(secret_ref, secret_value)
            .map_err(|_| SecretBackendError)
    }

    fn load(&self, secret_ref: &str) -> Result<Option<String>, SecretBackendError> {
        self.0.load(secret_ref).map_err(|_| SecretBackendError)
    }

    fn delete(&self, secret_ref: &str) -> Result<(), SecretBackendError> {
        self.0
            .delete(secret_ref)
            .map(|_| ())
            .map_err(|_| SecretBackendError)
    }
}

#[test]
fn encrypted_vault_resumes_the_maximum_import_batch() {
    let root = temp_root("vault-batch");
    let vault_root = root.join("vault");
    let secrets = VaultSecrets(Arc::new(
        crate::local_pool::store::vault::Vault::open(&vault_root, [7; 32]).unwrap(),
    ));
    let content = serde_json::Value::Array(
        (0..MAX_IMPORT_ITEMS)
            .map(|index| {
                serde_json::json!({
                    "auth_mode": "apikey",
                    "OPENAI_API_KEY": format!("synthetic-secret-{index}"),
                    "base_url": format!("https://provider-{index}.example.test/v1")
                })
            })
            .collect(),
    )
    .to_string();
    let store = ImportSessionStore::new(root.clone(), secrets);
    let started = store.start(&content, None, &[]).unwrap();
    let resumed = store.resume(&started.session_id, &[]).unwrap();

    assert_eq!(resumed.preview.rows.len(), MAX_IMPORT_ITEMS);
    assert_eq!(resumed.items.len(), MAX_IMPORT_ITEMS);
    assert!(
        !String::from_utf8_lossy(&fs::read(vault_root.join("secrets.enc")).unwrap())
            .contains("synthetic-secret")
    );
    store.cancel(&started.session_id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn restart_resume_reparses_secret_and_updates_existing_state() {
    let root = temp_root("resume");
    let secrets = MemorySecrets::default();
    let first_store = ImportSessionStore::new(root.clone(), secrets.clone());
    let started = first_store
        .start(&fixture(), Some("session.user@example.test.json"), &[])
        .unwrap();
    let identity_key = started.items[0].identity_key.clone();
    let session_id = started.session_id;
    drop(first_store);

    let reopened = ImportSessionStore::new(root.clone(), secrets);
    let resumed = reopened.resume(&session_id, &[identity_key]).unwrap();
    assert_eq!(resumed.session_id, session_id);
    assert_eq!(resumed.items[0].secrets().api_key(), Some(API_KEY));
    assert!(resumed.preview.rows[0].existing);
    assert!(!resumed.preview.rows[0].default_selected);

    reopened.cancel(&session_id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_preview_and_exchanged_credentials_survive_restart() {
    let root = temp_root("prepared-resume");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets.clone());
    let started = store
        .start(r#"{"refresh_token":"refresh-original"}"#, None, &[])
        .unwrap();
    let original_item_id = started.preview.rows[0].item_id.clone();
    let mut final_preview = started.preview.clone();
    final_preview.rows[0].identity = "Account ••••1234".into();
    final_preview.rows[0].quota_status = zenith_relay_core::accounts::ImportQuotaStatus::Success;
    let prepared = store
            .prepare(
                &started.session_id,
                Some(r#"{"account_id":"provider-account","access_token":"access-exchanged","refresh_token":"refresh-original"}"#),
                final_preview.clone(),
                &[],
            )
            .unwrap();
    assert_eq!(prepared.preview, final_preview);
    assert_eq!(prepared.items[0].item_id, original_item_id);
    assert_eq!(
        prepared.items[0].secrets().access_token(),
        Some("access-exchanged")
    );

    let reopened = ImportSessionStore::new(root.clone(), secrets.clone());
    let resumed = reopened.resume(&started.session_id, &[]).unwrap();
    assert_eq!(resumed.preview, final_preview);
    assert_eq!(resumed.items[0].item_id, original_item_id);
    reopened.cancel(&started.session_id).unwrap();
    assert!(!secrets.contains(&prepared_secret_ref(&started.session_id)));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_preview_keeps_rejected_rows_without_recovery_error() {
    let root = temp_root("prepared-invalid-row");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets);
    let input = r#"[{"email":"same@example.test","access_token":"first"},{"email":"same@example.test","access_token":"second"}]"#;
    let started = store.start(input, None, &[]).unwrap();
    assert_eq!(started.items.len(), 1);
    assert_eq!(started.preview.rows.len(), 2);
    let prepared = store
        .prepare(
            &started.session_id,
            Some(r#"[{"email":"same@example.test","access_token":"first"}]"#),
            started.preview.clone(),
            &[],
        )
        .unwrap();
    assert_eq!(prepared.items.len(), 1);
    assert_eq!(prepared.preview.rows.len(), 2);
    store.cancel(&started.session_id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_preview_accepts_a_row_rejected_during_preparation() {
    let root = temp_root("prepared-newly-invalid-row");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets);
    let started = store
        .start(r#"{"refresh_token":"refresh"}"#, None, &[])
        .unwrap();
    let mut final_preview = started.preview.clone();
    let row = &mut final_preview.rows[0];
    row.status = zenith_relay_core::accounts::ImportPreviewStatus::Invalid;
    row.selectable = false;
    row.default_selected = false;
    row.error = Some(zenith_relay_core::accounts::ImportIssue {
        code: zenith_relay_core::accounts::ImportIssueCode::RefreshExchangeFailed,
        message: "refresh exchange failed".into(),
    });

    let prepared = store
        .prepare(&started.session_id, Some("[]"), final_preview.clone(), &[])
        .unwrap();
    assert!(prepared.items.is_empty());
    assert_eq!(prepared.preview, final_preview);
    let resumed = store.resume(&started.session_id, &[]).unwrap();
    assert!(resumed.items.is_empty());
    assert_eq!(resumed.preview, final_preview);
    store.cancel(&started.session_id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_preview_reuses_original_secret_when_credentials_are_unchanged() {
    let root = temp_root("prepared-reused-secret");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets.clone());
    let started = store
        .start(
            r#"{"email":"same@example.test","access_token":"access"}"#,
            None,
            &[],
        )
        .unwrap();
    let prepared = store
        .prepare(&started.session_id, None, started.preview.clone(), &[])
        .unwrap();
    assert!(prepared.prepared);
    assert!(!secrets.contains(&prepared_secret_ref(&started.session_id)));
    assert!(secrets.contains(&secret_ref(&started.session_id)));
    store.cancel(&started.session_id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancel_and_complete_clear_secret_before_snapshot() {
    let root = temp_root("clear");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets.clone());
    let canceled = store.start(&fixture(), None, &[]).unwrap();
    let canceled_ref = secret_ref(&canceled.session_id);
    assert!(secrets.contains(&canceled_ref));
    store.cancel(&canceled.session_id).unwrap();
    assert!(!secrets.contains(&canceled_ref));
    assert!(!snapshot_path(&root, &canceled.session_id).unwrap().exists());

    let completed = store.start(&fixture(), None, &[]).unwrap();
    let completed_ref = secret_ref(&completed.session_id);
    store.complete(&completed.session_id).unwrap();
    assert!(!secrets.contains(&completed_ref));
    assert!(!snapshot_path(&root, &completed.session_id)
        .unwrap()
        .exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn expired_sessions_clear_snapshots_and_secrets() {
    let root = temp_root("expired");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets.clone());
    let first = store.start(&fixture(), None, &[]).unwrap();
    let second = store.start(&fixture(), None, &[]).unwrap();
    store
        .prepare(&first.session_id, None, first.preview.clone(), &[])
        .unwrap();

    assert_eq!(
        store
            .cleanup_expired_at(first.created_at_ms + IMPORT_SESSION_TTL_MS - 1)
            .unwrap(),
        0
    );
    assert_eq!(store.cleanup_expired_at(u64::MAX).unwrap(), 2);
    for session_id in [&first.session_id, &second.session_id] {
        assert!(!snapshot_path(&root, session_id).unwrap().exists());
        assert!(!prepared_snapshot_path(&root, session_id).unwrap().exists());
        assert!(!secrets.contains(&secret_ref(session_id)));
        assert!(!secrets.contains(&prepared_secret_ref(session_id)));
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn corrupt_unsafe_and_traversal_snapshots_are_rejected() {
    let root = temp_root("unsafe");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets);
    let started = store.start(&fixture(), None, &[]).unwrap();
    let path = snapshot_path(&root, &started.session_id).unwrap();
    fs::write(&path, b"{not-json").unwrap();
    assert_eq!(
        store.resume(&started.session_id, &[]).unwrap_err().code,
        ImportSessionErrorCode::SnapshotInvalid
    );
    assert_eq!(
        store.resume("../unsafe", &[]).unwrap_err().code,
        ImportSessionErrorCode::InvalidSessionId
    );
    store.cancel(&started.session_id).unwrap();

    let started = store.start(&fixture(), None, &[]).unwrap();
    let path = snapshot_path(&root, &started.session_id).unwrap();
    let mut snapshot: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    snapshot["preview"]["access_token"] = Value::String(ACCESS_TOKEN.to_string());
    fs::write(&path, serde_json::to_vec_pretty(&snapshot).unwrap()).unwrap();
    assert_eq!(
        store.resume(&started.session_id, &[]).unwrap_err().code,
        ImportSessionErrorCode::SnapshotUnsafe
    );
    store.cancel(&started.session_id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn snapshot_failure_rolls_back_secret_and_delete_failure_is_retryable() {
    let root = temp_root("rollback");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets.clone());
    let imports = root.join("imports");
    fs::create_dir_all(&imports).unwrap();
    fs::create_dir(imports.join("11111111-2222-4333-8444-555555555555.tmp")).unwrap();
    let error = store
        .start_with_id(FIXED_ID, &fixture(), None, &[])
        .unwrap_err();
    assert_eq!(error.code, ImportSessionErrorCode::SnapshotIo);
    assert!(!secrets.contains(&secret_ref(FIXED_ID)));
    fs::remove_dir_all(imports.join("11111111-2222-4333-8444-555555555555.tmp")).unwrap();

    let started = store.start(&fixture(), None, &[]).unwrap();
    secrets.state().fail_delete = true;
    assert_eq!(
        store.cancel(&started.session_id).unwrap_err().code,
        ImportSessionErrorCode::SecretStoreUnavailable
    );
    assert!(snapshot_path(&root, &started.session_id).unwrap().exists());
    secrets.state().fail_delete = false;
    store.cancel(&started.session_id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn serialized_snapshot_contains_only_redacted_preview_and_reference() {
    let root = temp_root("redacted");
    let secrets = MemorySecrets::default();
    let store = ImportSessionStore::new(root.clone(), secrets);
    let started = store
        .start(&fixture(), Some("session.user@example.test.json"), &[])
        .unwrap();
    let snapshot = fs::read_to_string(snapshot_path(&root, &started.session_id).unwrap()).unwrap();
    for forbidden in [
        API_KEY,
        ACCESS_TOKEN,
        EMAIL,
        "OPENAI_API_KEY",
        "access_token",
    ] {
        assert!(!snapshot.contains(forbidden));
    }
    assert!(snapshot.contains("secretRef"));
    assert!(snapshot.contains("preview"));
    store.cancel(&started.session_id).unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn secret_save_failure_never_creates_snapshot() {
    let root = temp_root("save-failure");
    let secrets = MemorySecrets::default();
    secrets.state().fail_save = true;
    let store = ImportSessionStore::new(root.clone(), secrets);
    let error = store
        .start_with_id(FIXED_ID, &fixture(), None, &[])
        .unwrap_err();
    assert_eq!(error.code, ImportSessionErrorCode::SecretStoreUnavailable);
    assert!(!snapshot_path(&root, FIXED_ID).unwrap().exists());
    fs::remove_dir_all(root).unwrap();
}

fn fixture() -> String {
    format!(
        r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{API_KEY}","access_note":"{ACCESS_TOKEN}","email":"{EMAIL}"}}"#
    )
}

fn temp_root(label: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-session-{label}-{}",
        Uuid::new_v4()
    ));
    fs::create_dir_all(&root).unwrap();
    root
}
