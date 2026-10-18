use super::*;
use crate::test_fixtures::{synthetic_server_account, test_app_state};

fn pending_import() -> PendingImport {
    PendingImport {
        id: "import_synthetic_expiry".into(),
        preview_json: "{}".into(),
        secret_ref: "account:synthetic-import".into(),
        created_at_ms: 0,
    }
}

#[test]
fn expired_snapshot_cannot_delete_a_consumed_account_credential() {
    let root = tempfile::TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let pending = pending_import();
    state.store.save_pending_import(&pending).unwrap();
    state
        .vault
        .save(&pending.secret_ref, "synthetic-credential")
        .unwrap();
    let mut account = synthetic_server_account("account-synthetic");
    account.secret_ref = pending.secret_ref.clone();
    state
        .store
        .save_account_and_consume_pending_import(&account, &pending.id)
        .unwrap();

    discard_expired_import(&state, &pending).unwrap();

    assert!(state.store.account(&account.id).unwrap().is_some());
    assert_eq!(
        state.vault.load(&account.secret_ref).unwrap().as_deref(),
        Some("synthetic-credential")
    );
}

#[test]
fn expired_snapshot_cannot_delete_a_replaced_pending_import() {
    let root = tempfile::TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let pending = pending_import();
    state.store.save_pending_import(&pending).unwrap();
    let replacement = PendingImport {
        created_at_ms: now_ms(),
        ..pending.clone()
    };
    state.store.save_pending_import(&replacement).unwrap();
    state
        .vault
        .save(&replacement.secret_ref, "synthetic-replacement")
        .unwrap();

    discard_expired_import(&state, &pending).unwrap();

    let retained = state
        .store
        .pending_import(&replacement.id)
        .unwrap()
        .unwrap();
    assert_eq!(retained.created_at_ms, replacement.created_at_ms);
    assert_eq!(
        state
            .vault
            .load(&replacement.secret_ref)
            .unwrap()
            .as_deref(),
        Some("synthetic-replacement")
    );
}

#[test]
fn current_expired_import_removes_only_its_pending_credential() {
    let root = tempfile::TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let pending = pending_import();
    state.store.save_pending_import(&pending).unwrap();
    state
        .vault
        .save(&pending.secret_ref, "synthetic-expired")
        .unwrap();
    state
        .vault
        .save("account:unrelated", "synthetic-unrelated")
        .unwrap();

    discard_expired_import(&state, &pending).unwrap();

    assert!(state.store.pending_import(&pending.id).unwrap().is_none());
    assert!(state.vault.load(&pending.secret_ref).unwrap().is_none());
    assert_eq!(
        state.vault.load("account:unrelated").unwrap().as_deref(),
        Some("synthetic-unrelated")
    );
}

#[test]
fn late_import_failure_does_not_change_a_newer_account_configuration() {
    let root = tempfile::TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let imported = synthetic_server_account("account-synthetic");
    state.store.save_account(&imported).unwrap();
    let (_, import_fence) = state.store.account_refresh_scope(&imported.id).unwrap();
    let mut updated = imported.clone();
    updated.enabled = false;
    updated.proxy_id = Some("proxy-synthetic-new".into());
    state.store.save_account(&updated).unwrap();

    mark_import_failure(&state, Some(&import_fence), "metadata_refresh_failed");

    let retained = state.store.account(&imported.id).unwrap().unwrap();
    assert_eq!(retained.health, AccountHealthState::Healthy);
    assert!(retained.last_error_code.is_none());
    assert!(!retained.enabled);
    assert_eq!(retained.proxy_id, updated.proxy_id);
}

#[test]
fn generic_import_failure_does_not_hide_a_current_terminal_account_failure() {
    let root = tempfile::TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let imported = synthetic_server_account("account-synthetic");
    state.store.save_account(&imported).unwrap();
    state
        .store
        .update_account(&imported.id, |account| {
            account.health = AccountHealthState::Blocked;
            account.last_error_code = Some("deactivated_workspace".into());
            Ok(())
        })
        .unwrap();

    let (_, current_fence) = state.store.account_refresh_scope(&imported.id).unwrap();
    mark_import_failure(&state, Some(&current_fence), "metadata_refresh_failed");

    let retained = state.store.account(&imported.id).unwrap().unwrap();
    assert_eq!(retained.health, AccountHealthState::Blocked);
    assert_eq!(
        retained.last_error_code.as_deref(),
        Some("deactivated_workspace")
    );
}

#[test]
fn current_import_followup_failure_is_retained_when_no_specific_failure_exists() {
    let root = tempfile::TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let imported = synthetic_server_account("account-synthetic");
    state.store.save_account(&imported).unwrap();
    let (_, fence) = state.store.account_refresh_scope(&imported.id).unwrap();

    mark_import_failure(&state, Some(&fence), "runtime_rebuild_failed");

    let retained = state.store.account(&imported.id).unwrap().unwrap();
    assert_eq!(retained.health, AccountHealthState::Degraded);
    assert_eq!(
        retained.last_error_code.as_deref(),
        Some("runtime_rebuild_failed")
    );
}
