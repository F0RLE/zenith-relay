use super::*;
use crate::local_pool::store::retired_accounts::{STATE_OWNERSHIP_OPERATION, STATE_REMOTE_TARGET};
use serde_json::{json, Value};

#[test]
fn retired_server_state_is_archived_and_pending_accounts_are_quarantined() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let mut affected = account_record("account-pending");
    affected.account.enabled = true;
    affected.account.in_pool = true;
    let untouched = account_record("account-untouched");
    store.upsert_account(affected.clone()).unwrap();
    store.upsert_account(untouched.clone()).unwrap();
    let target = json!({ "origin": "https://retired.example.test", "secretRef": "remote:test" });
    let operation = json!({
        "serverId": "server-old", "kind": "move_to_remote", "phase": "move_remote_applying",
        "localAccountIds": ["account-pending"], "remoteAccountIds": [], "updatedAtMs": 123
    });
    store
        .database()
        .replace_state_json(&[
            (STATE_REMOTE_TARGET, target.to_string()),
            (STATE_OWNERSHIP_OPERATION, operation.to_string()),
        ])
        .unwrap();
    drop(store);

    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let saved = store.account("account-pending").unwrap();
    assert!(!saved.account.enabled);
    assert!(!saved.account.in_pool);
    assert!(saved.recovery.is_some());
    let mut expected = affected;
    expected.account.enabled = false;
    expected.account.in_pool = false;
    expected.recovery = saved.recovery.clone();
    assert_eq!(saved, &expected);
    assert_eq!(store.account("account-untouched"), Some(&untouched));
    let archive: Value = serde_json::from_str(
        &store
            .database()
            .state_json("retired_server_state")
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(archive, json!({ "target": target, "operation": operation }));
    assert_eq!(
        store
            .database()
            .state_json(STATE_OWNERSHIP_OPERATION)
            .unwrap()
            .as_deref(),
        Some("null")
    );
    assert_eq!(
        store
            .database()
            .state_json(STATE_REMOTE_TARGET)
            .unwrap()
            .as_deref(),
        Some("null")
    );

    assert!(store
        .recover_account_locally("account-pending", false)
        .is_err());
    assert!(store.account("account-pending").unwrap().recovery.is_some());
    store
        .recover_account_locally("account-pending", true)
        .unwrap();
    let recovered = store.account("account-pending").unwrap();
    assert!(recovered.recovery.is_none());
    assert!(!recovered.account.enabled);
    assert!(!recovered.account.in_pool);
    drop(store);

    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert!(reopened
        .account("account-pending")
        .unwrap()
        .recovery
        .is_none());
    assert_eq!(reopened.account("account-untouched"), Some(&untouched));
    assert_eq!(
        serde_json::from_str::<Value>(
            &reopened
                .database()
                .state_json("retired_server_state")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        archive
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn saved_server_ownership_cannot_enable_local_routing_without_confirmation() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let mut account = account_record("account-owned");
    account.account.enabled = true;
    account.account.in_pool = true;
    account.recovery = Some(json!({ "reason": "recovery_required" }));
    store.upsert_account(account).unwrap();
    drop(store);
    let store = LocalPoolStore::open(root.clone()).unwrap();
    let account = store.account("account-owned").unwrap();
    assert!(!account.account.enabled);
    assert!(!account.account.in_pool);
    assert!(account.recovery.is_some());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_retired_ownership_is_preserved_without_a_partial_archive() {
    let root = temp_root();
    let store = LocalPoolStore::open(root.clone()).unwrap();
    let invalid = json!({ "serverId": "server-old", "localAccountIds": 123 }).to_string();
    let database = store.database();
    database
        .replace_state_json(&[(STATE_OWNERSHIP_OPERATION, invalid.clone())])
        .unwrap();
    drop(store);
    let error = LocalPoolStore::open(root.clone()).err().unwrap();
    assert_eq!(error.code, ErrorCode::RecoveryRequired);
    assert_eq!(
        database.state_json(STATE_OWNERSHIP_OPERATION).unwrap(),
        Some(invalid)
    );
    assert!(database
        .state_json("retired_server_state")
        .unwrap()
        .is_none());
    drop(database);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn legacy_account_owner_is_kept_as_private_recovery_evidence() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store.upsert_account(account_record("account-old")).unwrap();
    let mut saved = serde_json::to_value(account_record("account-old")).unwrap();
    let owner = json!({ "serverId": "server-old", "remoteAccountId": "external-old" });
    saved["remoteLocation"] = owner.clone();
    saved["account"]["enabled"] = json!(true);
    saved["account"]["inPool"] = json!(true);
    store
        .database()
        .replace_state_json(&[(STATE_ACCOUNTS, json!([saved]).to_string())])
        .unwrap();
    drop(store);

    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let account = store.account("account-old").unwrap();
    assert_eq!(account.recovery.as_ref(), Some(&owner));
    assert!(!account.account.enabled);
    assert!(!account.account.in_pool);
    let saved: Value = serde_json::from_str(
        &store
            .database()
            .state_json(STATE_ACCOUNTS)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert!(saved[0].get("remoteLocation").is_none());
    assert_eq!(saved[0]["recovery"], owner);
    assert!(store.recover_account_locally("account-old", false).is_err());
    store.recover_account_locally("account-old", true).unwrap();
    let account = store.account("account-old").unwrap();
    assert!(account.recovery.is_none());
    assert!(!account.account.enabled);
    assert!(!account.account.in_pool);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
