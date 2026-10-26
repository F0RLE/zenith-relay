use super::*;

fn seeded() -> (PathBuf, LocalPoolStore, rusqlite::Connection) {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store
        .replace_accounts_and_keys(
            vec![account_record("a"), account_record("b")],
            vec![LocalGatewayKeyRecord {
                id: "key".into(),
                label: "Synthetic".into(),
                enabled: true,
                system: false,
                secret_ref: "key:synthetic".into(),
                created_at: "2026-10-25T00:00:00Z".into(),
                last_used_at: None,
            }],
        )
        .unwrap();
    store
        .upsert_source(crate::local_pool::models::synthetic_responses_source())
        .unwrap();
    let connection = rusqlite::Connection::open(root.join("data/database/relay.sqlite")).unwrap();
    (root, store, connection)
}

#[test]
fn usage_items_survive_restart_without_rewriting_inventory_or_refresh_identity() {
    let (root, mut store, connection) = seeded();
    let base: String = connection
        .query_row(
            "SELECT value_json FROM app_state WHERE key='accounts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let changes = store.refresh_changes();
    let (mut account, fence) = store.account_refresh_scope("a").unwrap();
    account.account.last_used_at_ms = Some(123);
    account.account.quota.updated_at_ms = Some(123);
    store
        .persist_account_usage(
            account.clone(),
            "key",
            "2026-10-25T00:00:01Z",
            store.automations().clone(),
        )
        .unwrap();
    assert!(!changes.has_changed().unwrap());
    store.ensure_account_refresh_current(&fence).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT value_json FROM app_state WHERE key='accounts'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        base
    );
    let mut b = store.account("b").unwrap().clone();
    b.account.last_used_at_ms = Some(456);
    store
        .persist_account_usage(
            b.clone(),
            "key",
            "2026-10-25T00:00:02Z",
            store.automations().clone(),
        )
        .unwrap();
    let source_id = store.sources()[0].id.clone();
    store
        .touch_usage("key", &source_id, "2026-10-25T00:00:03Z")
        .unwrap();
    drop(store);
    let mut reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(reopened.account("a"), Some(&account));
    assert_eq!(reopened.account("b"), Some(&b));
    assert_eq!(
        reopened.keys()[0].last_used_at.as_deref(),
        Some("2026-10-25T00:00:03Z")
    );
    assert_eq!(
        reopened.sources()[0].last_used_at,
        reopened.keys()[0].last_used_at
    );
    // Reordering/replacing inventory folds observations into the new baseline.
    reopened
        .replace_accounts_and_keys(vec![b, account], reopened.keys().to_vec())
        .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM app_state_items WHERE state_key='accounts'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    reopened
        .replace_accounts_and_keys(vec![account_record("a")], reopened.keys().to_vec())
        .unwrap();
    drop(reopened);
    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(reopened.accounts().len(), 1);
    assert_eq!(reopened.account("a").unwrap().account.last_used_at_ms, None);
    drop(reopened);
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_usage_item_commit_preserves_memory_and_all_persisted_items() {
    let (root, mut store, connection) = seeded();
    let original = store.account("a").unwrap().clone();
    connection.execute_batch("CREATE TRIGGER fail_usage_key BEFORE INSERT ON app_state_items WHEN NEW.state_key='keys' BEGIN SELECT RAISE(ABORT, 'synthetic failure'); END;").unwrap();
    let mut next = original.clone();
    next.account.last_used_at_ms = Some(123);
    assert!(
        store
            .persist_account_usage(
                next,
                "key",
                "2026-10-25T00:00:01Z",
                store.automations().clone()
            )
            .is_err()
    );
    assert_eq!(store.account("a"), Some(&original));
    assert_eq!(store.keys()[0].last_used_at, None);
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM app_state_items", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    connection
        .execute_batch("DROP TRIGGER fail_usage_key;")
        .unwrap();
    let mut next = original.clone();
    next.account.enabled = !next.account.enabled;
    assert!(
        store
            .persist_account_usage(
                next,
                "key",
                "2026-10-25T00:00:01Z",
                store.automations().clone()
            )
            .is_err()
    );
    let mut next = original;
    next.account.auth_state =
        AccountAuthState::RequiresReauth(zenith_relay_core::accounts::ReauthReason::InvalidGrant);
    let changes = store.refresh_changes();
    store
        .persist_account_usage(
            next,
            "key",
            "2026-10-25T00:00:02Z",
            store.automations().clone(),
        )
        .unwrap();
    assert!(changes.has_changed().unwrap());
    drop(store);
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn state_items_upgrade_preserves_the_previous_inventory() {
    let (root, store, connection) = seeded();
    let before = store.accounts().to_vec();
    drop(store);
    connection
        .execute_batch(
            "DROP TABLE app_state_items;
             DROP TABLE weekly_capacity_history;
             DROP TABLE weekly_capacity_anchors;
             PRAGMA user_version = 39;",
        )
        .unwrap();
    let store = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(store.accounts(), before);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_schema
                 WHERE type='table' AND name IN
                 ('app_state_items', 'weekly_capacity_history', 'weekly_capacity_anchors')",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        3,
    );
    drop(store);
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}

#[test]
#[ignore = "paired synthetic inventory-write timing; no real accounts or credentials"]
fn measure_usage_inventory_writes() {
    for mode in ["full_inventory", "single_item"] {
        let mut samples = Vec::new();
        for _ in 0..3 {
            let root = temp_root();
            let mut store = LocalPoolStore::open(root.clone()).unwrap();
            let accounts = (0..64)
                .map(|index| {
                    let mut account = account_record(&format!("synthetic-{index}"));
                    account.models = (0..200)
                        .map(|model| format!("synthetic-model-{model}"))
                        .collect();
                    account
                })
                .collect::<Vec<_>>();
            store
                .replace_accounts_and_keys(accounts, Vec::new())
                .unwrap();
            let started = std::time::Instant::now();
            for sequence in 1..=64 {
                let mut account = store.accounts()[0].clone();
                account.account.last_used_at_ms = Some(sequence);
                if mode == "single_item" {
                    store
                        .persist_account_usage(
                            account,
                            "unused-key",
                            "2026-10-25T00:00:00Z",
                            store.automations().clone(),
                        )
                        .unwrap();
                } else {
                    let mut accounts = store.accounts().to_vec();
                    accounts[0] = account;
                    store
                        .replace_account_state(accounts, Vec::new(), store.automations().clone())
                        .unwrap();
                }
            }
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
            drop(store);
            let store = LocalPoolStore::open(root.clone()).unwrap();
            assert_eq!(store.accounts()[0].account.last_used_at_ms, Some(64));
            drop(store);
            fs::remove_dir_all(root).unwrap();
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "usage_inventory mode={mode} accounts=64 models_per_account=200 writes=64 median_ms={:.2}",
            samples[1]
        );
    }
}
