use super::*;

#[tokio::test]
async fn startup_reconciles_quota_persisted_before_listener_creation() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-startup-quota-{id}"));
    let account_id = format!("account_startup_{id}");
    let state = DesktopState::open(root.clone()).unwrap();
    let now_ms = current_time_ms();
    let credentials = StoredCodexCredentials::new(
        &account_id,
        "access-startup".into(),
        Some("refresh-startup".into()),
        Some("id-startup".into()),
        Some(now_ms.saturating_add(60_000)),
        now_ms,
        1,
        None,
        Some(format!("provider-{id}")),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    let credentials_store = CredentialStore::from_backend(NativeSecretBackend);
    credentials_store.save(&credentials).unwrap();
    let mut account = records::new_account_record(
        &credentials,
        zenith_relay_core::accounts::AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        now_ms,
    )
    .unwrap();
    account.account.in_pool = true;
    account.account.quota = zenith_relay_core::quota::QuotaSnapshot {
        limit_reached: true,
        updated_at_ms: Some(now_ms),
        ..Default::default()
    };
    state
        .store()
        .unwrap()
        .upsert_account(account.clone())
        .unwrap();

    // Build the same stale runtime that can be captured while a startup
    // quota refresh is still writing its result to the store.
    let stale_runtime = runtime_from_store(&state).await.unwrap();
    assert!(
        !stale_runtime
            .candidate_runtime_order()
            .into_iter()
            .find(|candidate| candidate.candidate_id == account_id)
            .expect("startup account candidate")
            .available
    );
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    state.gateway.start(stale_runtime, port).await.unwrap();

    account.account.quota = zenith_relay_core::quota::QuotaSnapshot {
        limit_reached: true,
        available_credits_micro_units: Some(123),
        provider_credits_available: true,
        updated_at_ms: Some(now_ms.saturating_add(1)),
        ..Default::default()
    };
    state.store().unwrap().upsert_account(account).unwrap();
    sync_running_account_states(&state).await.unwrap();

    assert!(
        state
            .gateway
            .runtime()
            .await
            .unwrap()
            .candidate_runtime_order()
            .into_iter()
            .find(|candidate| candidate.candidate_id == account_id)
            .expect("reconciled account candidate")
            .available
    );

    let key_secret_ref = state
        .store()
        .unwrap()
        .keys()
        .iter()
        .find(|key| key.system)
        .map(|key| key.secret_ref.clone());
    state.gateway.stop().await;
    credentials_store.delete(&account_id).unwrap();
    if let Some(secret_ref) = key_secret_ref {
        secret_store::delete(&secret_ref).unwrap();
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn persisted_reauth_disables_only_its_running_pool_account() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("zenith-relay-persisted-reauth-{id}"));
    let broken_account_id = format!("account-reauth-{id}");
    let healthy_account_id = format!("account-healthy-{id}");
    let now_ms = current_time_ms();
    let state = DesktopState::open(root.clone()).unwrap();
    let credentials_store = CredentialStore::from_backend(NativeSecretBackend);
    let credentials = |account_id: &str, provider_account_id: &str| {
        StoredCodexCredentials::new(
            account_id,
            "synthetic-access".into(),
            Some("synthetic-refresh".into()),
            Some("synthetic-id".into()),
            Some(now_ms.saturating_add(60_000)),
            now_ms,
            1,
            None,
            Some(provider_account_id.to_string()),
            None,
            None,
            Some("plus".into()),
            false,
        )
        .unwrap()
    };
    let broken_credentials = credentials(&broken_account_id, "provider-reauth");
    let healthy_credentials = credentials(&healthy_account_id, "provider-healthy");
    credentials_store.save(&broken_credentials).unwrap();
    credentials_store.save(&healthy_credentials).unwrap();

    let account = |credentials: &StoredCodexCredentials| {
        let mut account = records::new_account_record(
            credentials,
            zenith_relay_core::accounts::AccountAuthMode::OAuth,
            vec!["gpt-test".into()],
            0,
            now_ms,
        )
        .unwrap();
        account.account.in_pool = true;
        account
    };
    state
        .store()
        .unwrap()
        .upsert_account(account(&broken_credentials))
        .unwrap();
    state
        .store()
        .unwrap()
        .upsert_account(account(&healthy_credentials))
        .unwrap();

    let runtime = runtime_from_store(&state).await.unwrap();
    state.gateway.start(runtime, 0).await.unwrap();
    state
        .account_metadata_sink()
        .persist_auth_state(
            &broken_account_id,
            zenith_relay_core::accounts::AccountAuthState::RequiresReauth(
                zenith_relay_core::accounts::ReauthReason::InvalidatedRefreshToken,
            ),
        )
        .await
        .unwrap();
    let candidates = state
        .gateway
        .runtime()
        .await
        .unwrap()
        .candidate_runtime_order();
    let broken_available = candidates
        .iter()
        .find(|candidate| candidate.candidate_id == broken_account_id)
        .expect("reauth candidate")
        .available;
    let healthy_available = candidates
        .iter()
        .find(|candidate| candidate.candidate_id == healthy_account_id)
        .expect("healthy candidate")
        .available;
    let persisted_auth_state = state
        .store()
        .unwrap()
        .account(&broken_account_id)
        .expect("persisted reauth account")
        .account
        .auth_state;
    let key_secret_ref = state
        .store()
        .unwrap()
        .keys()
        .iter()
        .find(|key| key.system)
        .map(|key| key.secret_ref.clone());

    state.gateway.stop().await;
    credentials_store.delete(&broken_account_id).unwrap();
    credentials_store.delete(&healthy_account_id).unwrap();
    if let Some(secret_ref) = key_secret_ref {
        secret_store::delete(&secret_ref).unwrap();
    }
    drop(state);
    std::fs::remove_dir_all(root).unwrap();

    assert_eq!(
        persisted_auth_state,
        zenith_relay_core::accounts::AccountAuthState::RequiresReauth(
            zenith_relay_core::accounts::ReauthReason::InvalidatedRefreshToken,
        )
    );
    assert!(!broken_available);
    assert!(healthy_available);
}
