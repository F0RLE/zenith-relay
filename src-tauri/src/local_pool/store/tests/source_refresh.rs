use super::*;

fn source(id: &str) -> ProviderSourceRecord {
    serde_json::from_value(serde_json::json!({
        "id": id, "name": "Synthetic", "enabled": true, "inPool": false,
        "baseUrl": "https://provider.example.test/v1", "secretRef": format!("source:{id}"),
        "wireApi": "responses", "models": ["test"], "lastTestAt": null,
        "lastTestStatus": null, "lastError": null
    }))
    .unwrap()
}

#[test]
fn source_revisions_survive_reopen_and_merge_only_observations() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    let mut record = source("source-one");
    store.upsert_source(record.clone()).unwrap();
    let (_, first) = store.source_refresh_scope(&record.id).unwrap();
    assert_eq!(
        store
            .source_refresh_scope(&record.id)
            .ok()
            .map(|(_, fence)| fence.revision()),
        Some(first.identity().auth_revision)
    );
    let mut changes = store.refresh_changes();
    store
        .apply_source_refresh(&first, |value| {
            value.models = vec!["observed".into()];
            value.last_test_status = Some("ok".into());
            Ok(())
        })
        .unwrap();
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    assert_eq!(store.source_refresh_scope(&record.id).unwrap().1, first);
    record = store.source(&record.id).unwrap().clone();
    record.priority = 7;
    store.upsert_source(record.clone()).unwrap();
    assert_eq!(store.source_refresh_scope(&record.id).unwrap().1, first);
    record.base_url = "https://changed.example.test/v1".into();
    store.upsert_source(record.clone()).unwrap();
    assert!(store.ensure_source_refresh_current(&first).is_err());
    store.upsert_source(source("source-one")).unwrap();
    assert!(store.ensure_source_refresh_current(&first).is_err());
    let (_, restored) = store.source_refresh_scope("source-one").unwrap();
    store.replace_records(Vec::new(), Vec::new()).unwrap();
    store.upsert_source(source("source-one")).unwrap();
    let (_, readded) = store.source_refresh_scope("source-one").unwrap();
    assert_ne!(restored, readded);
    let revision_json = store
        .database
        .state_json(STATE_SOURCE_REVISIONS)
        .unwrap()
        .unwrap();
    assert!(!revision_json.contains("provider.example"));
    assert!(!revision_json.contains("secret"));
    drop(store);
    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(
        reopened.source_refresh_scope("source-one").unwrap().1,
        readded
    );
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn manual_catalog_and_explicit_credential_replacement_retire_both_kinds() {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store.upsert_source(source("source-one")).unwrap();
    let (_, first) = store.source_refresh_scope("source-one").unwrap();
    store.invalidate_source_refresh("source-one").unwrap();
    assert!(store.ensure_source_refresh_current(&first).is_err());
    let (_, second) = store.source_refresh_scope("source-one").unwrap();
    let mut record = store.source("source-one").unwrap().clone();
    record.last_test_status = Some("manual".into());
    store.upsert_source(record).unwrap();
    assert!(store.ensure_source_refresh_current(&second).is_err());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
