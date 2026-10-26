use super::*;
use zenith_relay_core::{protocol::PoolMembershipInput, IMAGE_API_MODEL};

async fn close_fixture(state: DesktopState, runtime: std::sync::Arc<GatewayRuntime>) {
    let database = std::sync::Arc::downgrade(&state.telemetry);
    state.gateway.stop().await;
    drop(runtime);
    drop(state);
    // The usage writer and snapshot performance jobs release SQLite asynchronously.
    // Reopen only after the old fixture has actually relinquished the database.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while database.strong_count() != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("fixture database must close before simulating a restart");
}

#[tokio::test]
async fn codex_text_opt_in_rebuilds_live_routes_without_enrolling_or_removing_accounts() {
    use crate::local_pool::commands::gateway::codex_text_models::apply_codex_text_models;

    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("relay-codex-text-{id}"));
    let account_id = format!("account_text_{id}");
    let now = current_time_ms();
    let credentials = StoredCodexCredentials::new(
        &account_id,
        "synthetic-access".into(),
        Some("synthetic-refresh".into()),
        None,
        Some(now + 60_000),
        now,
        1,
        None,
        Some(format!("synthetic-provider-{id}")),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    let credentials_store = CredentialStore::from_backend(NativeSecretBackend);
    credentials_store.save(&credentials).unwrap();
    let state = DesktopState::open(root.clone()).unwrap();
    let account = records::new_account_record(
        &credentials,
        zenith_relay_core::accounts::AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        now,
    )
    .unwrap();
    state
        .store()
        .unwrap()
        .upsert_account(account.clone())
        .unwrap();
    let runtime = runtime_from_store(&state).await.unwrap();
    let key = state.store().unwrap().keys()[0].clone();
    let secret = secret_store::load(&key.secret_ref).unwrap().unwrap();
    state.gateway.start(runtime.clone(), 0).await.unwrap();
    let address = state.gateway.address().await.unwrap();
    let mut gateway = state.store().unwrap().gateway().clone();
    gateway.port = address.port();
    state.store().unwrap().replace_gateway(gateway).unwrap();
    let input = |enabled, expected, accepted| {
        serde_json::from_value(serde_json::json!({
            "enabled": enabled, "expectedEnabled": expected, "riskAccepted": accepted
        }))
        .unwrap()
    };
    assert!(apply_codex_text_models(input(true, false, false), &state)
        .await
        .is_err());
    assert!(!state.store().unwrap().gateway().codex_text_models_enabled);
    apply_codex_text_models(input(true, false, true), &state)
        .await
        .unwrap();
    assert_eq!(
        state.store().unwrap().account(&account_id).unwrap(),
        &account
    );
    let visible = |runtime: &GatewayRuntime| {
        runtime.visible_models_for_secret(&secret, &[WireApi::Responses], current_time_ms())
    };
    assert!(visible(&state.gateway.runtime().await.unwrap()).is_empty());
    {
        let _setup = state.setup_guard().await;
        pool::apply_local_pool_membership(
            PoolMembershipInput {
                account_ids: vec![account_id.clone()],
                source_ids: Vec::new(),
                in_pool: true,
            },
            &state,
        )
        .await
        .unwrap();
    }
    let member = state.store().unwrap().account(&account_id).unwrap().clone();
    for (enabled, expected) in [(true, true), (false, true), (true, false)] {
        apply_codex_text_models(input(enabled, expected, enabled), &state)
            .await
            .unwrap();
        let current = state.gateway.runtime().await.unwrap();
        let models = visible(&current);
        assert_eq!(models.iter().any(|model| model == "gpt-test"), enabled);
        assert!(models.iter().any(|model| model == IMAGE_API_MODEL));
        assert!(current.block_degraded_routes_enabled());
        assert_eq!(
            pool::canonical_pool_model(&state, "gpt-test").is_ok(),
            enabled
        );
        assert!(pool::canonical_pool_model(&state, IMAGE_API_MODEL).is_ok());
        assert_eq!(state.gateway.address().await.unwrap(), address);
        assert_eq!(
            state.store().unwrap().account(&account_id).unwrap(),
            &member
        );
        let summary = crate::local_pool::commands::state::build_local_runtime_state(&state)
            .await
            .unwrap();
        assert_eq!(summary.gateway.codex_text_models_enabled, enabled);
        assert!(summary.gateway.block_degraded_routes_enabled);
        let summary_account = summary
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .unwrap();
        assert_eq!(
            summary_account
                .models
                .iter()
                .any(|model| model == "gpt-test"),
            enabled
        );
        assert!(current.update_account_models(&account_id, &["gpt-refreshed".into()]));
        assert_eq!(
            visible(&current)
                .iter()
                .any(|model| model == "gpt-refreshed"),
            enabled
        );
    }
    close_fixture(state, runtime).await;
    let state = DesktopState::open(root.clone()).unwrap();
    assert!(state.store().unwrap().gateway().codex_text_models_enabled);
    assert_eq!(
        state.store().unwrap().account(&account_id).unwrap(),
        &member
    );
    let runtime = runtime_from_store(&state).await.unwrap();
    assert!(visible(&runtime).iter().any(|model| model == "gpt-test"));
    assert!(credentials_store.load(&account_id).unwrap().is_some());
    close_fixture(state, runtime).await;
    credentials_store.delete(&account_id).unwrap();
    secret_store::delete(&key.secret_ref).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn explicit_codex_image_membership_survives_restart_and_revokes_live_access() {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let root = std::env::temp_dir().join(format!("relay-image-membership-{id}"));
    let account_id = format!("account_image_{id}");
    let now = current_time_ms();
    let credentials = StoredCodexCredentials::new(
        &account_id,
        "synthetic-access".into(),
        Some("synthetic-refresh".into()),
        None,
        Some(now + 60_000),
        now,
        1,
        None,
        Some(format!("synthetic-provider-{id}")),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .unwrap();
    let credentials_store = CredentialStore::from_backend(NativeSecretBackend);
    credentials_store.save(&credentials).unwrap();
    let state = DesktopState::open(root.clone()).unwrap();
    let account = records::new_account_record(
        &credentials,
        zenith_relay_core::accounts::AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        now,
    )
    .unwrap();
    assert!(!account.account.in_pool);
    assert!(!account.image_pool_enabled);
    state.store().unwrap().upsert_account(account).unwrap();
    let runtime = runtime_from_store(&state).await.unwrap();
    let key = state.store().unwrap().keys()[0].clone();
    let secret = secret_store::load(&key.secret_ref).unwrap().unwrap();
    let visible = |runtime: &GatewayRuntime| {
        runtime.visible_models_for_secret(&secret, &[WireApi::Responses], current_time_ms())
    };
    assert!(visible(&runtime).is_empty());
    state.gateway.start(runtime.clone(), 0).await.unwrap();
    {
        let _setup = state.setup_guard().await;
        let (snapshot, in_place, _) = pool::apply_local_pool_membership(
            PoolMembershipInput {
                account_ids: vec![account_id.clone()],
                source_ids: Vec::new(),
                in_pool: true,
            },
            &state,
        )
        .await
        .unwrap();
        assert!(in_place);
        let saved = state.store().unwrap().account(&account_id).unwrap().clone();
        assert!(saved.image_pool_enabled && saved.account.in_pool);
        assert_eq!(saved.effective_models(), &["gpt-test"]);
        let record = snapshot
            .accounts
            .iter()
            .find(|a| a.account.id == account_id)
            .unwrap();
        assert!(record.account.in_pool && record.image_pool_enabled);
        let view = crate::local_pool::commands::state::build_local_runtime_state(&state)
            .await
            .unwrap();
        let summary = view.accounts.iter().find(|a| a.id == account_id).unwrap();
        assert!(summary.in_pool);
        assert_eq!(summary.models, [IMAGE_API_MODEL]);
    }
    assert_eq!(visible(&runtime), [IMAGE_API_MODEL]);
    // Refreshes and editable model rules cannot turn an image member into a text member.
    assert!(runtime.update_account_models(&account_id, &["gpt-new".into()]));
    let mut policy_account = state.store().unwrap().account(&account_id).unwrap().clone();
    policy_account.allowed_models = vec!["gpt-new".into()];
    assert!(
        runtime.update_account_policy(&account_id, runtime_account_policy(&policy_account, now))
    );
    assert!(visible(&runtime).is_empty());
    policy_account.allowed_models.clear();
    assert!(
        runtime.update_account_policy(&account_id, runtime_account_policy(&policy_account, now))
    );
    assert_eq!(visible(&runtime), [IMAGE_API_MODEL]);
    close_fixture(state, runtime).await;

    let state = DesktopState::open(root.clone()).unwrap();
    let saved = state.store().unwrap().account(&account_id).unwrap().clone();
    assert!(saved.image_pool_enabled && saved.account.in_pool);
    let runtime = runtime_from_store(&state).await.unwrap();
    assert_eq!(visible(&runtime), [IMAGE_API_MODEL]);
    state.gateway.start(runtime.clone(), 0).await.unwrap();
    {
        let _setup = state.setup_guard().await;
        let (_, in_place, _) = pool::apply_local_pool_membership(
            PoolMembershipInput {
                account_ids: vec![account_id.clone()],
                source_ids: Vec::new(),
                in_pool: false,
            },
            &state,
        )
        .await
        .unwrap();
        assert!(in_place);
    }
    let saved = state.store().unwrap().account(&account_id).unwrap().clone();
    assert!(!saved.image_pool_enabled && !saved.account.in_pool);
    assert!(visible(&runtime).is_empty());
    assert_eq!(saved.effective_models(), &["gpt-test"]);
    assert!(credentials_store.load(&account_id).unwrap().is_some());
    close_fixture(state, runtime).await;
    credentials_store.delete(&account_id).unwrap();
    secret_store::delete(&key.secret_ref).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
