use super::*;

fn current_scope(store: &LocalPoolStore) -> AccountRefreshScope {
    let (initial_account, fence) = store.account_refresh_scope("test-account").unwrap();
    AccountRefreshScope {
        initial_account,
        fence,
        started_at_ms: 200,
    }
}

#[test]
fn empty_siwc_catalog_withdraws_models_and_recovers_only_discovery_state() {
    with_store(|store, scope| {
        let mut account = scope.initial_account.clone();
        account.discovered_models = Some(vec!["gpt-old".into()]);
        account.account.in_pool = true;
        account.account.health = AccountHealthState::Degraded;
        account.account.last_error_code = Some("models_transport".into());
        store.upsert_account(account.clone()).unwrap();
        let scope = current_scope(store);

        let applied =
            apply_models_read(store, &scope, OAuthClientKind::Siwc, Ok(Vec::new())).unwrap();

        assert!(applied.refresh_result);
        assert_eq!(applied.account.models, account.models);
        assert_eq!(applied.account.discovered_models, Some(Vec::new()));
        assert_eq!(applied.account.pool_models(false).count(), 0);
        assert_eq!(
            applied.account.account.auth_state,
            account.account.auth_state
        );
        assert_eq!(applied.account.account.health, AccountHealthState::Healthy);
        assert!(applied.account.account.last_error_code.is_none());
        assert_eq!(applied.account.account.quota, account.account.quota);
        assert_eq!(applied.account.account.in_pool, account.account.in_pool);
        assert_eq!(applied.account.account.enabled, account.account.enabled);
        assert_eq!(store.account("test-account"), Some(&applied.account));

        let encoded = serde_json::to_string(&applied.account).unwrap();
        let restored: LocalAccountRecord = serde_json::from_str(&encoded).unwrap();
        assert!(restored.effective_models().is_empty());
        let scope = current_scope(store);
        let repeated =
            apply_models_read(store, &scope, OAuthClientKind::Siwc, Ok(Vec::new())).unwrap();
        assert!(!repeated.refresh_result);
        assert!(repeated.account.effective_models().is_empty());
    });
}

#[test]
fn siwc_discovery_failures_preserve_the_last_observed_catalog_even_when_empty() {
    for models in [vec!["gpt-available".to_string()], Vec::new()] {
        for (code, retryable, http_status) in [
            (ModelDiscoveryFailureCode::Transport, true, None),
            (ModelDiscoveryFailureCode::Forbidden, false, Some(403)),
            (ModelDiscoveryFailureCode::InvalidResponse, false, Some(200)),
        ] {
            with_store(|store, scope| {
                let mut account = scope.initial_account.clone();
                account.discovered_models = Some(models.clone());
                store.upsert_account(account.clone()).unwrap();
                let scope = current_scope(store);
                let failure = ModelDiscoveryFailure {
                    code,
                    retryable,
                    http_status,
                    retry_after_ms: None,
                };

                let applied =
                    apply_models_read(store, &scope, OAuthClientKind::Siwc, Err(failure)).unwrap();

                assert!(!applied.refresh_result);
                assert_eq!(applied.account.effective_models(), models);
                assert_eq!(applied.account.models, account.models);
                assert_eq!(
                    applied.account.account.auth_state,
                    account.account.auth_state
                );
                assert_eq!(applied.account.account.in_pool, account.account.in_pool);
                assert_eq!(
                    applied.account.account.last_error_code.as_deref(),
                    Some(code.management_code())
                );
            });
        }
    }
}

#[test]
fn siwc_catalog_reduction_and_recovery_keep_only_the_current_ordered_models() {
    with_store(|store, scope| {
        for models in [
            vec!["gpt-second".to_string(), "gpt-first".to_string()],
            vec!["gpt-first".to_string()],
            Vec::new(),
            vec!["gpt-recovered".to_string()],
        ] {
            let current = current_scope(store);
            let applied =
                apply_models_read(store, &current, OAuthClientKind::Siwc, Ok(models.clone()))
                    .unwrap();
            assert!(applied.refresh_result);
            assert_eq!(applied.account.effective_models(), models);
            assert_eq!(applied.account.models, scope.initial_account.models);
        }
    });
}

#[test]
fn empty_siwc_catalog_preserves_a_newer_authentication_failure() {
    with_store(|store, scope| {
        let mut account = scope.initial_account.clone();
        account.account.auth_state = AccountAuthState::RequiresReauth(
            zenith_relay_core::accounts::ReauthReason::InvalidGrant,
        );
        account.account.health = AccountHealthState::Unhealthy;
        account.account.last_error_code = Some("invalid_grant".into());
        store.upsert_account(account.clone()).unwrap();

        let applied =
            apply_models_read(store, scope, OAuthClientKind::Siwc, Ok(Vec::new())).unwrap();

        assert!(applied.refresh_result);
        assert!(applied.account.effective_models().is_empty());
        assert_eq!(
            applied.account.account.auth_state,
            account.account.auth_state
        );
        assert_eq!(applied.account.account.health, account.account.health);
        assert_eq!(
            applied.account.account.last_error_code,
            account.account.last_error_code
        );
    });
}

#[test]
fn empty_native_catalog_keeps_its_existing_discovery_policy() {
    for client_kind in [OAuthClientKind::Codex, OAuthClientKind::ExcelBps] {
        with_store(|store, scope| {
            let applied = apply_models_read(store, scope, client_kind, Ok(Vec::new())).unwrap();
            assert!(!applied.refresh_result);
            assert_eq!(applied.account, scope.initial_account);
        });
    }
}
