use super::*;

fn seeded() -> (PathBuf, LocalPoolStore) {
    let root = temp_root();
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store.upsert_account(account_record("account-a")).unwrap();
    store.upsert_account(account_record("account-b")).unwrap();
    (root, store)
}

#[test]
fn registration_events_exclude_usage_but_include_auth_eligibility_and_bulk_replacement() {
    let (root, mut store) = seeded();
    let mut changes = store.refresh_changes();
    let (mut account, original_fence) = store.account_refresh_scope("account-a").unwrap();
    account.account.quota.updated_at_ms = Some(12);
    account.account.last_used_at_ms = Some(12);
    account.account.token_generation += 1;
    account.account.in_pool = !account.account.in_pool;
    store.upsert_account(account.clone()).unwrap();
    assert!(!changes.has_changed().unwrap());
    account.account.auth_state =
        AccountAuthState::RequiresReauth(zenith_relay_core::accounts::ReauthReason::InvalidGrant);
    store.upsert_account(account.clone()).unwrap();
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    store
        .ensure_account_refresh_current(&original_fence)
        .unwrap();
    account.account.auth_state = AccountAuthState::Active;
    store
        .replace_accounts_keys_and_ownership_operation(
            vec![account, store.account("account-b").unwrap().clone()],
            Vec::new(),
            None,
        )
        .unwrap();
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    store.invalidate_refresh_configuration().unwrap();
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    store.invalidate_account_refresh(&["account-a"]).unwrap();
    assert!(changes.has_changed().unwrap());
    changes.borrow_and_update();
    store.replace_accounts_and_keys(vec![], vec![]).unwrap();
    assert!(changes.has_changed().unwrap());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_refresh_revisions_survive_reopen_and_do_not_contain_credentials() {
    let (root, mut store) = seeded();
    let (_, before) = store.account_refresh_scope("account-a").unwrap();
    store.invalidate_account_refresh(&["account-a"]).unwrap();
    let (_, after) = store.account_refresh_scope("account-a").unwrap();
    assert_ne!(before, after);
    let json = store
        .database
        .state_json(STATE_REFRESH_REVISIONS)
        .unwrap()
        .unwrap();
    assert!(!json.contains("secret"));
    assert!(!json.contains("identity"));
    assert!(!json.contains("proxy"));
    drop(store);
    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    assert_eq!(
        reopened.account_refresh_scope("account-a").unwrap().1,
        after
    );
    assert!(reopened.ensure_account_refresh_current(&before).is_err());
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_scope_changes_retire_reads_even_if_the_record_is_restored() {
    let changes: [fn(&mut LocalAccountRecord); 7] = [
        |account| account.account.enabled = false,
        |account| account.account.source_id = "different-source".into(),
        |account| account.account.identity = account_record("other").account.identity,
        |account| account.account.auth_mode = AccountAuthMode::ImportedToken,
        |account| account.account.secret_refs = vec!["different-secret-ref".into()],
        |account| account.account.created_at_ms += 1,
        |account| {
            account.remote_location = Some(zenith_relay_core::protocol::RemoteAccountLocation {
                server_id: "test-server".into(),
                remote_account_id: "test-remote-account".into(),
            })
        },
    ];
    for change in changes {
        let (root, mut store) = seeded();
        let (original, fence) = store.account_refresh_scope("account-a").unwrap();
        let (_, unrelated) = store.account_refresh_scope("account-b").unwrap();
        let mut changed = original.clone();
        change(&mut changed);
        store.upsert_account(changed).unwrap();
        assert!(store.ensure_account_refresh_current(&fence).is_err());
        store.upsert_account(original).unwrap();
        assert!(store.ensure_account_refresh_current(&fence).is_err());
        store.ensure_account_refresh_current(&unrelated).unwrap();
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn quota_models_usage_and_token_rotation_do_not_change_the_scope() {
    let (root, mut store) = seeded();
    let (mut account, fence) = store.account_refresh_scope("account-a").unwrap();
    account.account.token_generation += 1;
    account.account.token_updated_at_ms = Some(200);
    account.account.last_used_at_ms = Some(200);
    account.account.quota.updated_at_ms = Some(200);
    account.account.subscription.plan_type = Some("plus".into());
    account.account.health = AccountHealthState::Degraded;
    account.account.auth_state = AccountAuthState::Error;
    account.account.label = "Renamed".into();
    account.account.in_pool = false;
    account.account.draining = true;
    account.priority = 8;
    account.weight = 9;
    account.allowed_models = vec!["gpt-test".into()];
    account.discovered_models = Some(vec!["gpt-test-new".into()]);
    account.client_auth_status = Some("signed-in".into());
    store.upsert_account(account.clone()).unwrap();
    store.ensure_account_refresh_current(&fence).unwrap();
    let applied = store
        .apply_account_refresh(&fence, |current| {
            current.account.quota.updated_at_ms = Some(300);
            Ok(())
        })
        .unwrap();
    assert_eq!(applied.previous, account);
    assert_eq!(applied.account.account.label, "Renamed");
    assert_eq!(applied.account.weight, 9);
    assert_eq!(applied.account.account.token_generation, 2);
    assert_eq!(applied.account.discovered_models, account.discovered_models);
    store.ensure_account_refresh_current(&fence).unwrap();
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn delete_and_readd_do_not_reuse_an_incarnation_across_restarts() {
    let (root, mut store) = seeded();
    let (account, fence) = store.account_refresh_scope("account-a").unwrap();
    store
        .replace_accounts_and_keys(vec![account_record("account-b")], Vec::new())
        .unwrap();
    assert!(store.apply_account_refresh(&fence, |_| Ok(())).is_err());
    drop(store);
    let mut store = LocalPoolStore::open(root.clone()).unwrap();
    store.upsert_account(account).unwrap();
    assert!(store.apply_account_refresh(&fence, |_| Ok(())).is_err());
    assert_ne!(store.account_refresh_scope("account-a").unwrap().1, fence);
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn ownership_transaction_and_reset_invalidate_account_reads() {
    let (root, mut store) = seeded();
    let (account, fence) = store.account_refresh_scope("account-a").unwrap();
    store
        .replace_accounts_keys_and_ownership_operation(Vec::new(), Vec::new(), None)
        .unwrap();
    store
        .replace_accounts_keys_and_ownership_operation(vec![account.clone()], Vec::new(), None)
        .unwrap();
    assert!(store.ensure_account_refresh_current(&fence).is_err());
    let (_, next) = store.account_refresh_scope("account-a").unwrap();
    store.reset_local_records().unwrap();
    store.upsert_account(account).unwrap();
    assert!(store.ensure_account_refresh_current(&next).is_err());
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn relevant_gateway_changes_and_secret_backed_proxy_changes_retire_reads() {
    let changes: [fn(&mut GatewaySettings); 3] = [
        |gateway| gateway.common_proxy_configured = true,
        |gateway| gateway.account_proxy_required = true,
        |gateway| gateway.quota_request_timeout_seconds = 10,
    ];
    for change in changes {
        let (root, mut store) = seeded();
        let (_, fence) = store.account_refresh_scope("account-a").unwrap();
        let original = store.gateway().clone();
        let mut changed = original.clone();
        change(&mut changed);
        store.replace_gateway(changed).unwrap();
        store.replace_gateway(original).unwrap();
        assert!(store.ensure_account_refresh_current(&fence).is_err());
        let (_, next) = store.account_refresh_scope("account-a").unwrap();
        store.invalidate_refresh_configuration().unwrap();
        assert!(store.ensure_account_refresh_current(&next).is_err());
        drop(store);
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn display_and_inference_settings_do_not_invalidate_monitoring() {
    let (root, mut store) = seeded();
    let (_, fence) = store.account_refresh_scope("account-a").unwrap();
    let mut gateway = store.gateway().clone();
    gateway.enabled = !gateway.enabled;
    gateway.max_retry_candidates = 5;
    gateway.hidden_models = vec!["gpt-test".into()];
    gateway.catalog_refresh_error = Some("test-warning".into());
    store.replace_gateway(gateway).unwrap();
    store.ensure_account_refresh_current(&fence).unwrap();
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn account_and_revision_changes_commit_or_fail_together() {
    let (root, mut store) = seeded();
    let (original, fence) = store.account_refresh_scope("account-a").unwrap();
    let connection = rusqlite::Connection::open(root.join("data/database/relay.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_account_write BEFORE UPDATE ON app_state WHEN NEW.key = 'accounts' BEGIN SELECT RAISE(ABORT, 'test write failure'); END;").unwrap();
    let mut changed = original.clone();
    changed.account.enabled = false;
    assert!(store.upsert_account(changed).is_err());
    assert_eq!(store.account("account-a"), Some(&original));
    store.ensure_account_refresh_current(&fence).unwrap();
    drop(store);
    drop(connection);
    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    reopened.ensure_account_refresh_current(&fence).unwrap();
    assert_eq!(reopened.account("account-a"), Some(&original));
    drop(reopened);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn refresh_callbacks_cannot_change_account_configuration() {
    let (root, mut store) = seeded();
    let (original, fence) = store.account_refresh_scope("account-a").unwrap();
    assert!(store
        .apply_account_refresh(&fence, |account| {
            account.account.enabled = false;
            Ok(())
        })
        .is_err());
    assert!(store
        .apply_account_refresh(&fence, |account| {
            account.account.id = "replacement".into();
            Ok(())
        })
        .is_err());
    assert_eq!(store.account("account-a"), Some(&original));
    drop(store);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn revision_upgrade_is_automatic_but_malformed_revisions_are_not_silently_reset() {
    let (root, store) = seeded();
    let connection = rusqlite::Connection::open(root.join("data/database/relay.sqlite")).unwrap();
    connection
        .execute(
            "DELETE FROM app_state WHERE key = ?1",
            [STATE_REFRESH_REVISIONS],
        )
        .unwrap();
    drop(store);
    let store = LocalPoolStore::open(root.clone()).unwrap();
    store.account_refresh_scope("account-a").unwrap();
    store
        .database
        .replace_state_json(&[(
            STATE_REFRESH_REVISIONS,
            r#"{"clock":0,"configuration":0,"accounts":{"account-a":3}}"#.into(),
        )])
        .unwrap();
    drop(store);
    assert!(matches!(
        LocalPoolStore::open(root.clone()),
        Err(LocalPoolError {
            code: ErrorCode::RecoveryRequired,
            ..
        })
    ));
    drop(connection);
    fs::remove_dir_all(root).unwrap();
}
