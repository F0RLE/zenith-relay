use super::*;
use crate::test_fixtures::{pooled_source, test_app_state};
use tempfile::TempDir;

async fn fixture() -> (TempDir, Arc<AppState>, GatewayKeyRecord, GatewayKeyRecord) {
    let root = TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let source = pooled_source("key-source", "test-model");
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    let current = state
        .store
        .keys()
        .unwrap()
        .into_iter()
        .find(|key| key.id == SYSTEM_GATEWAY_KEY_ID)
        .unwrap();
    state
        .vault
        .save(&current.secret_ref, "synthetic-old-key")
        .unwrap();
    let pending = GatewayKeyRecord {
        id: format!("{PROFILE_KEY_ROTATION_PREFIX}synthetic"),
        label: "Synthetic pending key".into(),
        enabled: true,
        system: true,
        secret_ref: format!("key:{PROFILE_KEY_ROTATION_PREFIX}synthetic"),
        created_at_ms: now_ms(),
        last_used_at_ms: None,
    };
    state
        .vault
        .save(&pending.secret_ref, "synthetic-new-key")
        .unwrap();
    state.store.save_key(&pending).unwrap();
    state.rebuild_runtime().await.unwrap();
    (root, state, current, pending)
}

#[tokio::test]
async fn profile_key_commit_waits_for_build_before_changing_live_secret() {
    let (_root, state, current, pending) = fixture().await;
    let old_runtime = state.runtime().unwrap().unwrap();
    let old_build = state.lock_runtime_rebuild().await;
    let worker_state = state.clone();
    let rotation_id = pending.id.clone();
    let commit = tokio::spawn(async move {
        commit_profile_key_rotation(State(worker_state), Path(rotation_id)).await
    });
    tokio::task::yield_now().await;
    assert!(!commit.is_finished());
    assert_eq!(
        state.vault.load(&current.secret_ref).unwrap().as_deref(),
        Some("synthetic-old-key")
    );
    assert!(state
        .store
        .keys()
        .unwrap()
        .iter()
        .any(|key| key.id == pending.id));
    drop(old_build);

    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), commit)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        state.vault.load(&current.secret_ref).unwrap().as_deref(),
        Some("synthetic-new-key")
    );
    assert!(!state
        .store
        .keys()
        .unwrap()
        .iter()
        .any(|key| key.id == pending.id));
    assert!(old_runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    assert!(!Arc::ptr_eq(
        &old_runtime,
        &state.runtime().unwrap().unwrap()
    ));
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn profile_key_abort_waits_for_build_before_removing_pending_secret() {
    let (_root, state, current, pending) = fixture().await;
    let old_runtime = state.runtime().unwrap().unwrap();
    let old_build = state.lock_runtime_rebuild().await;
    let worker_state = state.clone();
    let rotation_id = pending.id.clone();
    let abort = tokio::spawn(async move {
        abort_profile_key_rotation(State(worker_state), Path(rotation_id)).await
    });
    tokio::task::yield_now().await;
    assert!(!abort.is_finished());
    assert_eq!(
        state.vault.load(&pending.secret_ref).unwrap().as_deref(),
        Some("synthetic-new-key")
    );
    assert!(state
        .store
        .keys()
        .unwrap()
        .iter()
        .any(|key| key.id == pending.id));
    drop(old_build);

    assert_eq!(
        tokio::time::timeout(std::time::Duration::from_secs(5), abort)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        StatusCode::NO_CONTENT
    );
    assert!(state.vault.load(&pending.secret_ref).unwrap().is_none());
    assert!(!state
        .store
        .keys()
        .unwrap()
        .iter()
        .any(|key| key.id == pending.id));
    assert_eq!(
        state.vault.load(&current.secret_ref).unwrap().as_deref(),
        Some("synthetic-old-key")
    );
    assert!(old_runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    assert!(!Arc::ptr_eq(
        &old_runtime,
        &state.runtime().unwrap().unwrap()
    ));
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn failed_profile_key_restore_keeps_the_old_runtime_retired() {
    let (root, state, current, pending) = fixture().await;
    let old_runtime = state.runtime().unwrap().unwrap();
    // An atomic vault write cannot complete while the backup destination
    // is a directory. Both commit and restoration then fail closed.
    let backup = root.path().join("vault/secrets.enc.bak");
    if backup.is_file() {
        std::fs::remove_file(&backup).unwrap();
    }
    std::fs::create_dir(&backup).unwrap();
    assert!(
        commit_profile_key_rotation(State(state.clone()), Path(pending.id.clone()))
            .await
            .is_err()
    );
    assert!(state.runtime().unwrap().is_none());
    assert!(old_runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    assert_eq!(
        state.vault.load(&current.secret_ref).unwrap().as_deref(),
        Some("synthetic-old-key")
    );
    assert!(state
        .store
        .keys()
        .unwrap()
        .iter()
        .any(|key| key.id == pending.id));
    std::fs::remove_dir(&backup).unwrap();
    state.shutdown_runtime().await.unwrap();
}
