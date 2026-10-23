use super::*;

#[test]
fn trash_survives_restart_and_restore_keeps_the_account_disabled() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let record = account_record("synthetic-trash");
    store.upsert_account(record.clone()).unwrap();
    let (_, old_refresh) = store.account_refresh_scope(&record.account.id).unwrap();
    store
        .trash_accounts(
            std::slice::from_ref(&record.account.id),
            AutomationRecords::default(),
            123,
        )
        .unwrap();
    assert!(store.accounts().is_empty());
    assert_eq!(store.account_trash()[0].record, record);
    assert!(store.ensure_account_refresh_current(&old_refresh).is_err());
    drop(store);
    let mut reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(reopened.account_trash()[0].trashed_at_ms, 123);
    reopened
        .restore_trashed_accounts(std::slice::from_ref(&record.account.id))
        .unwrap();
    assert!(reopened.account_trash().is_empty());
    let restored = reopened.account(&record.account.id).unwrap();
    assert!(!restored.account.enabled);
    assert!(!restored.account.in_pool);
    assert_eq!(restored.account.secret_refs, record.account.secret_refs);
    assert_eq!(restored.models, record.models);
    assert!(reopened
        .ensure_account_refresh_current(&old_refresh)
        .is_err());
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn trash_batch_validates_every_id_and_prevents_import_over_the_saved_account() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let record = account_record("synthetic-trash");
    store.upsert_account(record.clone()).unwrap();
    assert!(store
        .trash_accounts(
            &[record.account.id.clone(), "missing".into()],
            AutomationRecords::default(),
            1
        )
        .is_err());
    assert_eq!(store.accounts(), std::slice::from_ref(&record));
    assert!(store.account_trash().is_empty());
    store
        .trash_accounts(
            std::slice::from_ref(&record.account.id),
            AutomationRecords::default(),
            1,
        )
        .unwrap();
    assert!(store.upsert_account(record.clone()).is_err());
    assert!(store.accounts().is_empty());
    assert_eq!(store.account_trash()[0].record, record);
    store
        .purge_trashed_accounts(std::slice::from_ref(&record.account.id))
        .unwrap();
    assert!(store.account_trash().is_empty());
    store.upsert_account(record).unwrap();
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_trash_and_restore_writes_preserve_both_durable_inventories() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let record = account_record("synthetic-trash");
    store.upsert_account(record.clone()).unwrap();
    let connection = rusqlite::Connection::open(root.join("data/database/relay.sqlite")).unwrap();
    let reject_writes = || {
        connection
            .execute_batch(
                "CREATE TRIGGER reject_trash_write BEFORE INSERT ON app_state
         WHEN NEW.key = 'account_trash' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;",
            )
            .unwrap()
    };
    reject_writes();
    assert!(store
        .trash_accounts(
            std::slice::from_ref(&record.account.id),
            AutomationRecords::default(),
            1
        )
        .is_err());
    assert_eq!(store.accounts(), std::slice::from_ref(&record));
    assert!(store.account_trash().is_empty());
    connection
        .execute_batch("DROP TRIGGER reject_trash_write")
        .unwrap();
    store
        .trash_accounts(
            std::slice::from_ref(&record.account.id),
            AutomationRecords::default(),
            1,
        )
        .unwrap();
    reject_writes();
    assert!(store
        .restore_trashed_accounts(std::slice::from_ref(&record.account.id))
        .is_err());
    assert!(store.accounts().is_empty());
    assert_eq!(store.account_trash()[0].record, record);
    drop(store);
    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert!(reopened.accounts().is_empty());
    assert_eq!(reopened.account_trash()[0].record, record);
    drop(reopened);
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}
