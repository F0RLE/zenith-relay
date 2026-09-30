use super::*;

#[test]
fn unauthorized_access_only_account_is_saved_but_removed_from_routing() {
    let root = temp_root("usage-401");
    let account_id = format!("account-{}", uuid::Uuid::new_v4().simple());
    let state = DesktopState::open(root.clone()).unwrap();
    let mut account = account_record(&account_id);
    account.account.auth_state = AccountAuthState::DegradedAccessOnly;
    state.store().unwrap().upsert_account(account).unwrap();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    credentials
        .save(
            &StoredCodexCredentials::new(
                &account_id,
                "access-private".into(),
                None,
                None,
                Some(u64::MAX),
                1,
                1,
                None,
                Some("provider-private".into()),
                None,
                None,
                None,
                false,
            )
            .unwrap(),
        )
        .unwrap();
    assert!(credentials.require(&account_id).is_ok());
    let retry_at_ms = now_ms().saturating_add(30 * 60_000);

    (state.usage_callback())(account_status_event(
        &account_id,
        401,
        Some("*"),
        Some(retry_at_ms),
        1,
    ));

    let observed_after = now_ms();
    let stored = credentials.require(&account_id).unwrap();
    assert!(!stored.is_access_usable(observed_after, 0));
    let account = state.store().unwrap().account(&account_id).unwrap().clone();
    assert_eq!(account.account.auth_state, AccountAuthState::Error);
    assert_eq!(account.account.health, AccountHealthState::Unhealthy);
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("upstream_unauthorized")
    );
    assert!(account.cooldowns.is_empty());
    assert_eq!(account.consecutive_failures, 0);
    assert!(!state.refresh_started.load(Ordering::Acquire));
    drop(state);

    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    let account = reopened.account(&account_id).unwrap();
    assert!(account.cooldowns.is_empty());
    assert_eq!(account.consecutive_failures, 0);
    drop(reopened);
    credentials.delete(&account_id).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn delayed_unauthorized_does_not_expire_a_newer_oauth_login() {
    let root = temp_root("usage-delayed-401");
    let account_id = format!("account-{}", uuid::Uuid::new_v4().simple());
    let state = DesktopState::open(root.clone()).unwrap();
    let mut account = account_record(&account_id);
    account.account.token_generation = 2;
    state.store().unwrap().upsert_account(account).unwrap();
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    credentials
        .save(
            &StoredCodexCredentials::new(
                &account_id,
                "newer-access-private".into(),
                Some("newer-refresh-private".into()),
                Some("newer-id-private".into()),
                Some(u64::MAX),
                2,
                2,
                None,
                Some("provider-private".into()),
                None,
                None,
                None,
                false,
            )
            .unwrap(),
        )
        .unwrap();

    // This result belongs to the immediately preceding credential
    // generation, not the just-completed sign-in above.
    (state.usage_callback())(account_status_event(
        &account_id,
        401,
        Some("*"),
        Some(now_ms().saturating_add(30 * 60_000)),
        1,
    ));

    let stored = credentials.require(&account_id).unwrap();
    assert_eq!(stored.generation(), 2);
    assert!(stored.is_access_usable(now_ms(), 0));
    let account = state.store().unwrap().account(&account_id).unwrap().clone();
    assert_eq!(account.account.auth_state, AccountAuthState::Active);
    assert_eq!(account.account.health, AccountHealthState::Healthy);
    assert_eq!(account.account.last_error_code, None);

    drop(state);
    credentials.delete(&account_id).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn generic_forbidden_account_stays_available_until_an_actual_success() {
    let root = temp_root("usage-403");
    let account_id = "account-forbidden";
    let state = DesktopState::open(root.clone()).unwrap();
    state
        .store()
        .unwrap()
        .upsert_account(account_record(account_id))
        .unwrap();
    let retry_at_ms = now_ms().saturating_add(30 * 60_000);

    (state.usage_callback())(account_status_event(
        account_id,
        403,
        Some("*"),
        Some(retry_at_ms),
        2,
    ));
    {
        let store = state.store().unwrap();
        let account = store.account(account_id).unwrap();
        assert_eq!(account.account.health, AccountHealthState::Degraded);
        assert_eq!(
            account.account.last_error_code.as_deref(),
            Some("upstream_forbidden")
        );
        assert!(account.cooldowns.is_empty());
        assert_eq!(account.consecutive_failures, 0);
    }
    drop(state);

    let reopened = DesktopState::open(root.clone()).unwrap();
    assert_eq!(
        reopened
            .store()
            .unwrap()
            .account(account_id)
            .unwrap()
            .account
            .health,
        AccountHealthState::Degraded
    );
    (reopened.usage_callback())(account_success_event(account_id));
    let store = reopened.store().unwrap();
    let account = store.account(account_id).unwrap();
    assert_eq!(account.account.health, AccountHealthState::Healthy);
    assert_eq!(account.account.last_error_code, None);
    assert!(account.cooldowns.is_empty());
    assert_eq!(account.consecutive_failures, 0);
    drop(store);
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn explicit_workspace_disable_stays_blocked_until_an_actual_success() {
    let root = temp_root("usage-403-disabled");
    let account_id = "account-disabled";
    let state = DesktopState::open(root.clone()).unwrap();
    state
        .store()
        .unwrap()
        .upsert_account(account_record(account_id))
        .unwrap();
    let mut event = account_status_event(
        account_id,
        403,
        Some("*"),
        Some(now_ms().saturating_add(30 * 60_000)),
        2,
    );
    event.error_category = Some("deactivated_workspace".into());
    (state.usage_callback())(event);
    assert_eq!(
        state
            .store()
            .unwrap()
            .account(account_id)
            .unwrap()
            .account
            .health,
        AccountHealthState::Blocked
    );
    drop(state);

    let reopened = DesktopState::open(root.clone()).unwrap();
    let account = reopened
        .store()
        .unwrap()
        .account(account_id)
        .unwrap()
        .clone();
    assert_eq!(account.account.health, AccountHealthState::Blocked);
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("deactivated_workspace")
    );
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn opening_storage_clears_a_false_upstream_forbidden_block() {
    let root = temp_root("false-403-block");
    {
        let mut store = LocalPoolStore::open(root.clone()).unwrap();
        let mut false_block = account_record("false-block");
        false_block.account.health = AccountHealthState::Blocked;
        false_block.account.last_error_code = Some("upstream_forbidden".into());
        let mut real_block = account_record("real-block");
        real_block.account.health = AccountHealthState::Blocked;
        real_block.account.last_error_code = Some("deactivated_workspace".into());
        store.upsert_account(false_block).unwrap();
        store.upsert_account(real_block).unwrap();
    }

    let reopened = LocalPoolStore::open(root.clone()).unwrap();
    let false_block = reopened.account("false-block").unwrap();
    assert_eq!(false_block.account.health, AccountHealthState::Healthy);
    assert_eq!(false_block.account.last_error_code, None);
    let real_block = reopened.account("real-block").unwrap();
    assert_eq!(real_block.account.health, AccountHealthState::Blocked);
    assert_eq!(
        real_block.account.last_error_code.as_deref(),
        Some("deactivated_workspace")
    );
    drop(reopened);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn account_results_update_health_without_persisting_cooldowns() {
    let mut account = account_record("account-race");
    let failure = account_status_event("account-race", 429, Some("*"), Some(500), 2);
    assert!(apply_account_usage_state(
        &mut account,
        &failure,
        100,
        None,
        None,
        false,
    ));

    let mut late_success = account_success_event("account-race");
    late_success.consecutive_failures = None;
    assert!(!apply_account_usage_state(
        &mut account,
        &late_success,
        200,
        None,
        Some(AccountAuthState::Active),
        false,
    ));
    let older_failure = account_status_event("account-race", 429, Some("*"), Some(300), 1);
    assert!(apply_account_usage_state(
        &mut account,
        &older_failure,
        250,
        None,
        None,
        false,
    ));

    assert!(account.cooldowns.is_empty());
    assert_eq!(account.consecutive_failures, 0);
    assert_eq!(account.account.health, AccountHealthState::Degraded);
    assert_eq!(
        account.account.last_error_code.as_deref(),
        Some("upstream_rate_limited")
    );
}
#[test]
fn neutral_request_failure_does_not_degrade_the_account() {
    let mut account = account_record("account-neutral");
    let mut event = account_status_event("account-neutral", 400, None, None, 0);
    event.consecutive_failures = None;
    event.error_category = Some("response_affinity_miss".into());

    assert!(!apply_account_usage_state(
        &mut account,
        &event,
        100,
        None,
        None,
        false,
    ));
    assert_eq!(account.account.health, AccountHealthState::Healthy);
    assert_eq!(account.account.last_error_code, None);
    assert!(account.cooldowns.is_empty());
    assert_eq!(account.consecutive_failures, 0);
}
#[test]
fn model_entitlement_is_local_but_edge_challenges_degrade_the_account() {
    for (category, expected_health, expected_error) in [
        (
            "upstream_usage_not_included",
            AccountHealthState::Healthy,
            None,
        ),
        (
            "upstream_edge_challenge",
            AccountHealthState::Degraded,
            Some("upstream_edge_challenge"),
        ),
    ] {
        let mut account = account_record(category);
        let mut event = account_status_event(category, 403, Some("*"), Some(60_000), 1);
        event.error_category = Some(category.into());

        assert!(!apply_account_usage_state(
            &mut account,
            &event,
            100,
            None,
            None,
            false,
        ));
        assert_eq!(account.account.health, expected_health);
        assert_eq!(account.account.last_error_code.as_deref(), expected_error);
    }
}
#[test]
fn canonical_account_credential_controls_availability() {
    let mut account = account_record("account_credential");
    account.account.secret_refs.clear();

    assert!(!account_secret_available(&account, &MemorySecrets(HashMap::new())).unwrap());
    assert!(account_secret_available(
        &account,
        &MemorySecrets(HashMap::from([(
            "account:codex:account_credential".into(),
            "credential".into(),
        )]))
    )
    .unwrap());
}
