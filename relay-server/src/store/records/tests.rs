use super::*;
use crate::store::{test_support::test_root, PendingImport};
use std::fs;
use zenith_relay_core::accounts::{AccountAuthState, AccountHealthState};

fn account() -> ServerAccountRecord {
    ServerAccountRecord {
        id: "account_1".into(),
        label: "Account".into(),
        identity_hint: "account".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        source_id: "openai_codex".into(),
        secret_ref: "account:1".into(),
        provider_family: Some("openai".into()),
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        models: vec!["gpt-test".into()],
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        subscription: Default::default(),
        quota: Default::default(),
        purchase_cost_micro_usd: None,
        cooldowns: Default::default(),
        consecutive_failures: 0,
        created_at_ms: 1,
        last_used_at_ms: None,
        last_error_code: None,
        proxy_id: None,
        bypass_common_proxy: false,
    }
}

#[test]
fn account_observation_update_preserves_a_newer_proxy_configuration() {
    let root = test_root("account-observation-update");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    let mut configured = account();
    store.save_account(&configured).unwrap();
    configured.proxy_id = Some("proxy_1".into());
    store.save_account(&configured).unwrap();

    store
        .update_account("account_1", |record| {
            record.last_used_at_ms = Some(2);
            Ok(())
        })
        .unwrap()
        .unwrap();

    let stored = store.account("account_1").unwrap().unwrap();
    assert_eq!(stored.proxy_id.as_deref(), Some("proxy_1"));
    assert_eq!(stored.last_used_at_ms, Some(2));
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn usage_hint_identity_is_the_transaction_identity_not_a_later_configuration() {
    let root = test_root("usage-hint-identity");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    store.save_account(&account()).unwrap();
    let original = store
        .account_refresh_scope("account_1")
        .unwrap()
        .1
        .identity();
    let ((), observed) = store
        .update_account_with_refresh_identity("account_1", |record| {
            record.last_used_at_ms = Some(2);
            Ok(())
        })
        .unwrap()
        .unwrap();
    assert_eq!(observed, original);

    let mut updated = store.account("account_1").unwrap().unwrap();
    updated.proxy_id = Some("new-proxy".into());
    store.save_account(&updated).unwrap();
    let current = store
        .account_refresh_scope("account_1")
        .unwrap()
        .1
        .identity();
    assert_ne!(observed, current);
    assert_eq!(
        store.account("account_1").unwrap().unwrap().last_used_at_ms,
        Some(2)
    );
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_import_commit_consumes_its_pending_record_atomically() {
    let root = test_root("account-import-commit");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    let pending = PendingImport {
        id: "import_pending".into(),
        preview_json: "{}".into(),
        secret_ref: "account:pending".into(),
        created_at_ms: 1,
    };
    store.save_pending_import(&pending).unwrap();

    assert!(store
        .save_account_and_consume_pending_import(&account(), &pending.id)
        .unwrap());
    assert!(store.account("account_1").unwrap().is_some());
    assert!(store.pending_import(&pending.id).unwrap().is_none());

    let mut missing = account();
    missing.id = "account_missing".into();
    assert!(store
        .save_account_and_consume_pending_import(&missing, "import_missing")
        .is_err());
    assert!(store.account(&missing.id).unwrap().is_none());

    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn pool_membership_batch_rolls_back_when_one_record_is_missing() {
    let root = test_root("pool-membership-rollback");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    {
        let connection = store.lock().unwrap();
        connection
            .execute(
                "INSERT INTO sources(id, data_json, secret_ref) VALUES ('source_1', '{\"id\":\"source_1\",\"inPool\":false}', 'source:1')",
                [],
            )
            .unwrap();
    }

    assert!(store
        .replace_pool_membership(
            &[
                ("source_1".to_string(), true),
                ("missing".to_string(), true)
            ],
            &[],
        )
        .is_err());
    let in_pool: bool = store
        .lock()
        .unwrap()
        .query_row(
            "SELECT json_extract(data_json, '$.inPool') FROM sources WHERE id = 'source_1'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!in_pool);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}
