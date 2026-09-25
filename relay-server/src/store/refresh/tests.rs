use super::*;
use crate::store::test_support::test_root;

fn account() -> ServerAccountRecord {
    serde_json::from_value(serde_json::json!({
        "id": "synthetic", "label": "Synthetic", "identityHint": "synthetic",
        "enabled": true, "inPool": true, "draining": false, "sourceId": "openai_codex",
        "secretRef": "account:synthetic:login1", "authState": zenith_relay_core::accounts::AccountAuthState::Active, "health": "healthy",
        "models": ["test"], "allowedModels": [], "excludedModels": [], "priority": 0,
        "weight": 1, "subscription": zenith_relay_core::quota::Subscription::default(), "quota": zenith_relay_core::quota::QuotaSnapshot::default(), "cooldowns": {}, "consecutiveFailures": 0,
        "createdAtMs": 1
    })).unwrap()
}

#[test]
fn delayed_observations_merge_with_latest_policy_usage_and_other_kind() {
    let root = test_root("refresh-merge");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    store.save_account(&account()).unwrap();
    let (_, quota_fence) = store.account_refresh_scope("synthetic").unwrap();
    let (_, model_fence) = store.account_refresh_scope("synthetic").unwrap();
    store
        .update_account("synthetic", |current| {
            current.label = "Edited".into();
            current.weight = 7;
            current.in_pool = false;
            current.draining = true;
            current.last_used_at_ms = Some(200);
            current.cooldowns.insert("test".into(), 9_000);
            Ok(())
        })
        .unwrap();
    store
        .apply_account_refresh(&model_fence, |account| {
            account.discovered_models = Some(vec!["new".into()]);
            Ok(())
        })
        .unwrap();
    store
        .apply_account_refresh(&quota_fence, |account| {
            account.quota.updated_at_ms = Some(100);
            Ok(())
        })
        .unwrap();
    let current = store.account("synthetic").unwrap().unwrap();
    assert_eq!(current.label, "Edited");
    assert_eq!(current.weight, 7);
    assert!(!current.in_pool && current.draining);
    assert_eq!(current.last_used_at_ms, Some(200));
    assert_eq!(current.cooldowns["test"], 9_000);
    assert_eq!(current.discovered_models, Some(vec!["new".into()]));
    assert_eq!(current.quota.updated_at_ms, Some(100));
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn login_disable_proxy_and_remove_readd_fence_delayed_observations_including_aba() {
    for mutation in [
        "login",
        "enabled",
        "proxy",
        "common_proxy",
        "required",
        "delete",
        "readd",
        "aba",
    ] {
        let root = test_root("refresh-fence");
        let store = Store::open(root.join("relay.sqlite")).unwrap();
        let original = account();
        store.save_account(&original).unwrap();
        let (_, fence) = store.account_refresh_scope("synthetic").unwrap();
        match mutation {
            "common_proxy" => store.set_common_proxy_id(Some("proxy:new")).unwrap(),
            "required" => store.set_account_proxy_required(true).unwrap(),
            "delete" | "readd" => {
                store.delete_account("synthetic").unwrap();
                if mutation == "readd" {
                    store.save_account(&original).unwrap();
                }
            }
            _ => {
                let mut next = original.clone();
                match mutation {
                    "login" => next.secret_ref = "account:synthetic:login2".into(),
                    "enabled" | "aba" => next.enabled = false,
                    "proxy" => next.proxy_id = Some("proxy:new".into()),
                    _ => unreachable!(),
                }
                store.save_account(&next).unwrap();
                if mutation == "aba" {
                    store.save_account(&original).unwrap();
                }
            }
        }
        let result = store.apply_account_refresh(&fence, |_| -> Result<(), String> {
            panic!("stale apply: {mutation}")
        });
        assert!(result.is_err(), "{mutation}");
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn fences_survive_reopen_and_normal_quota_observations_do_not_change_identity() {
    let root = test_root("refresh-reopen");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    store.save_account(&account()).unwrap();
    let (_, before) = store.account_refresh_scope("synthetic").unwrap();
    store
        .update_account("synthetic", |account| {
            account.quota.updated_at_ms = Some(20);
            Ok(())
        })
        .unwrap();
    assert_eq!(store.account_refresh_scope("synthetic").unwrap().1, before);
    drop(store);
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    assert_eq!(store.account_refresh_scope("synthetic").unwrap().1, before);
    store.delete_account("synthetic").unwrap();
    store.save_account(&account()).unwrap();
    assert_ne!(store.account_refresh_scope("synthetic").unwrap().1, before);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn normalizing_absent_legacy_defaults_does_not_create_a_new_login_revision() {
    let root = test_root("refresh-legacy-defaults");
    let store = Store::open(root.join("relay.sqlite")).unwrap();
    let mut record = account();
    record.created_at_ms = 0;
    store.save_account(&record).unwrap();
    store.lock().unwrap().execute(
        "UPDATE accounts SET data_json = json_remove(data_json, '$.createdAtMs', '$.bypassCommonProxy') WHERE id = ?1",
        [&record.id],
    ).unwrap();
    let (_, before) = store.account_refresh_scope(&record.id).unwrap();
    store
        .apply_account_refresh(&before, |account| {
            account.quota.updated_at_ms = Some(20);
            Ok(())
        })
        .unwrap();
    assert_eq!(store.account_refresh_scope(&record.id).unwrap().1, before);
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
