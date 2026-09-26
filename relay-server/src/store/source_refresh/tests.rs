use super::*;
use crate::store::test_support::test_root;
use serde_json::json;

fn source(id: &str) -> SourceRecord {
    serde_json::from_value(json!({
        "id": id, "name": "Synthetic", "enabled": true, "inPool": false,
        "draining": false, "baseUrl": "https://provider.example.test/v1",
        "secretRef": format!("source:{id}"), "wireApi": "responses", "models": ["test"],
        "allowedModels": [], "excludedModels": [], "priority": 0, "weight": 1
    }))
    .unwrap()
}

#[test]
fn source_observation_keeps_revision_but_normal_edits_and_readd_retire_it() {
    let root = test_root("source-refresh");
    std::fs::create_dir_all(&root).unwrap();
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    let mut record = source("source-one");
    store.save_source(&record).unwrap();
    let (_, first) = store.source_refresh_scope(&record.id).unwrap();
    assert_eq!(
        store
            .source_refresh_scopes()
            .unwrap()
            .iter()
            .find(|(source, _)| source.id == record.id)
            .map(|(_, fence)| fence.revision()),
        Some(first.revision())
    );
    store
        .apply_source_refresh(&first, |value| {
            value.models = vec!["observed".into()];
            Ok(())
        })
        .unwrap();
    assert_eq!(store.source_refresh_scope(&record.id).unwrap().1, first);
    record.priority = 7;
    record.models = vec!["observed".into()];
    store.save_source(&record).unwrap();
    assert_eq!(store.source_refresh_scope(&record.id).unwrap().1, first);
    record.base_url = "https://other.example.test/v1".into();
    store.save_source(&record).unwrap();
    assert_ne!(store.source_refresh_scope(&record.id).unwrap().1, first);
    assert!(store.apply_source_refresh(&first, |_| Ok(())).is_err());
    let (_, changed) = store.source_refresh_scope(&record.id).unwrap();
    store.delete_source(&record.id).unwrap();
    store.save_source(&record).unwrap();
    let (_, readded) = store.source_refresh_scope(&record.id).unwrap();
    assert_ne!(changed, readded);
    assert_eq!(
        store
            .source_refresh_scopes()
            .unwrap()
            .iter()
            .find(|(source, _)| source.id == record.id)
            .map(|(_, fence)| fence.revision()),
        Some(readded.revision())
    );
    drop(store);
    let reopened = Store::open(root.join("relay.sqlite")).unwrap();
    assert_eq!(
        reopened.source_refresh_scope(&record.id).unwrap().1,
        readded
    );
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_secret_replacement_retires_even_an_identical_key_without_storing_it() {
    let root = test_root("source-key-fence");
    std::fs::create_dir_all(&root).unwrap();
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    store.save_source(&source("source-one")).unwrap();
    let (_, fence) = store.source_refresh_scope("source-one").unwrap();
    store.invalidate_source_refresh("source-one").unwrap();
    assert_ne!(store.source_refresh_scope("source-one").unwrap().1, fence);
    assert!(store.apply_source_refresh(&fence, |_| Ok(())).is_err());
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bulk_config_changes_and_failed_sql_transactions_do_not_resurrect_old_scopes() {
    let root = test_root("source-bulk-fence");
    std::fs::create_dir_all(&root).unwrap();
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    let mut first = source("first");
    let mut second = source("second");
    store
        .save_sources(&[first.clone(), second.clone()])
        .unwrap();
    let old_first = store.source_refresh_scope(&first.id).unwrap().1;
    let old_second = store.source_refresh_scope(&second.id).unwrap().1;
    first.base_url = "https://other.example.test/v1".into();
    second.name = "Display only".into();
    store.save_sources(&[first.clone(), second]).unwrap();
    let changed = store.source_refresh_scope(&first.id).unwrap().1;
    assert_ne!(old_first, changed);
    assert_eq!(old_second, store.source_refresh_scope("second").unwrap().1);
    assert!(store.apply_source_refresh(&old_first, |_| Ok(())).is_err());
    // The database and monotonic clock must both roll back after a malformed
    // configuration write fails inside a transaction.
    {
        let mut connection = store.lock().unwrap();
        let transaction = connection.transaction().unwrap();
        assert!(transaction
            .execute(
                "UPDATE sources SET data_json = '{bad-json}' WHERE id = ?1",
                [&first.id],
            )
            .is_err());
        transaction.rollback().unwrap();
    }
    assert_eq!(store.source_refresh_scope(&first.id).unwrap().1, changed);
    first.base_url = "https://provider.example.test/v1".into();
    store.save_source(&first).unwrap();
    let restored = store.source_refresh_scope(&first.id).unwrap().1;
    assert_ne!(restored, changed);
    assert_ne!(restored, old_first);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
