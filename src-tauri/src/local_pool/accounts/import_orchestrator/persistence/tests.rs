use super::*;
use crate::local_pool::accounts::records;
use crate::local_pool::commands::runtime_from_store;
use crate::local_pool::store::secret_store;
use zenith_relay_core::accounts::AccountAuthMode;

fn credentials(
    account_id: &str,
    suffix: &str,
    issued_at_ms: u64,
    generation: u64,
) -> StoredCodexCredentials {
    StoredCodexCredentials::new(
        account_id,
        format!("access-{suffix}"),
        Some(format!("refresh-{suffix}")),
        Some(format!("id-{suffix}")),
        Some(issued_at_ms.saturating_add(60_000)),
        issued_at_ms,
        generation,
        Some("private@example.test".into()),
        Some("provider-private".into()),
        None,
        None,
        Some("plus".into()),
        false,
    )
    .expect("synthetic credentials")
}

fn account(credentials: &StoredCodexCredentials) -> LocalAccountRecord {
    records::new_account_record(
        credentials,
        AccountAuthMode::OAuth,
        vec!["gpt-test".into()],
        0,
        credentials.issued_at_ms(),
    )
    .expect("synthetic account")
}

async fn cleanup_import_test_state(
    state: DesktopState,
    credential_store: CredentialStore<NativeSecretBackend>,
    account_id: &str,
    root: std::path::PathBuf,
) {
    state.gateway.stop().await;
    credential_store
        .delete(account_id)
        .expect("cleanup account");
    let key_refs = {
        let store = state.store().expect("store");
        store
            .keys()
            .iter()
            .map(|key| key.secret_ref.clone())
            .collect::<Vec<_>>()
    };
    for secret_ref in key_refs {
        secret_store::delete(&secret_ref).expect("cleanup key");
    }
    drop(state);
    std::fs::remove_dir_all(root).expect("cleanup state");
}

#[tokio::test]
async fn reimporting_identical_credentials_retires_preexisting_refresh_reads() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-refresh-fence-{}",
        uuid::Uuid::new_v4()
    ));
    let state = DesktopState::open(root.clone()).unwrap();
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
    let imported = credentials(&account_id, "same", 10, 1);
    let record = account(&imported);
    credential_store.save(&imported).unwrap();
    state
        .store()
        .unwrap()
        .upsert_account(record.clone())
        .unwrap();
    let (_, old_scope) = state
        .store()
        .unwrap()
        .account_refresh_scope(&account_id)
        .unwrap();
    {
        let _mutation = state.setup_guard().await;
        persist_imported_account(
            &state,
            &credential_store,
            &imported,
            Some(&imported),
            record,
        )
        .await
        .unwrap();
    }
    assert!(state
        .store()
        .unwrap()
        .ensure_account_refresh_current(&old_scope)
        .is_err());
    cleanup_import_test_state(state, credential_store, &account_id, root).await;
}

#[tokio::test]
async fn importing_an_account_outside_the_pool_keeps_the_running_gateway() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-no-pool-runtime-{}",
        uuid::Uuid::new_v4()
    ));
    let state = DesktopState::open(root.clone()).expect("state");
    let runtime = runtime_from_store(&state).await.expect("runtime");
    let address = state
        .gateway
        .start(runtime.clone(), 0)
        .await
        .expect("gateway");
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
    let imported = credentials(&account_id, "outside-pool", 10, 1);
    let record = account(&imported);

    persist_imported_account(&state, &credential_store, &imported, None, record)
        .await
        .expect("import");

    assert_eq!(state.gateway.address().await, Some(address));
    let running = state.gateway.runtime().await.expect("running runtime");
    assert!(std::sync::Arc::ptr_eq(&running, &runtime));
    drop(running);
    drop(runtime);

    cleanup_import_test_state(state, credential_store, &account_id, root).await;
}

#[tokio::test]
async fn importing_a_pool_account_restarts_the_gateway_without_losing_it() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-pool-runtime-{}",
        uuid::Uuid::new_v4()
    ));
    let state = DesktopState::open(root.clone()).expect("state");
    let runtime = runtime_from_store(&state).await.expect("runtime");
    let address = state.gateway.start(runtime, 0).await.expect("gateway");
    let mut gateway = state.store().expect("store").gateway().clone();
    gateway.port = address.port();
    state
        .store()
        .expect("store")
        .replace_gateway(gateway)
        .expect("gateway settings");
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
    let imported = credentials(&account_id, "pool", 10, 1);
    let mut record = account(&imported);
    record.account.in_pool = true;

    persist_imported_account(&state, &credential_store, &imported, None, record)
        .await
        .expect("import");

    assert_eq!(
        state.gateway.address().await.map(|value| value.port()),
        Some(address.port())
    );
    let running = state.gateway.runtime().await.expect("running runtime");
    assert!(running
        .candidate_runtime_order()
        .iter()
        .any(|candidate| candidate.candidate_id.starts_with("account_")));
    drop(running);
    cleanup_import_test_state(state, credential_store, &account_id, root).await;
}

#[test]
fn import_rollback_restores_only_its_own_durable_snapshot() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-rollback-{}",
        uuid::Uuid::new_v4()
    ));
    let state = DesktopState::open(root.clone()).expect("state");
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
    let previous_credentials = credentials(&account_id, "previous", 10, 1);
    let attempted_credentials = credentials(&account_id, "attempted", 20, 2);
    let previous_account = account(&previous_credentials);
    let attempted_account = account(&attempted_credentials);
    let unrelated_credentials = credentials("account_unrelated", "other", 30, 1);
    let unrelated_account = account(&unrelated_credentials);

    credential_store
        .save(&attempted_credentials)
        .expect("attempted secret");
    state
        .store()
        .expect("store")
        .upsert_account(attempted_account.clone())
        .expect("attempted account");
    state
        .store()
        .expect("store")
        .upsert_account(unrelated_account.clone())
        .expect("unrelated account");

    let commit = ImportedAccountCommit {
        account_id: account_id.clone(),
        previous_credentials: Some(previous_credentials.clone()),
        previous_account: Some(previous_account.clone()),
        attempted_credentials: attempted_credentials.clone(),
        attempted_account,
        runtime_sync_required: false,
    };

    assert!(
        restore_import_durable_state_if_current(&credential_store, &state, &commit)
            .expect("rollback")
    );
    assert!(credential_store
        .require(&account_id)
        .expect("restored secret")
        .matches_snapshot(&previous_credentials));
    let store = state.store().expect("store");
    assert_eq!(store.account(&account_id), Some(&previous_account));
    assert_eq!(store.account("account_unrelated"), Some(&unrelated_account));
    drop(store);

    credential_store
        .delete(&account_id)
        .expect("cleanup account secret");
    drop(state);
    std::fs::remove_dir_all(root).expect("cleanup state");
}

#[test]
fn import_rollback_never_replaces_a_newer_credential_snapshot() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-stale-rollback-{}",
        uuid::Uuid::new_v4()
    ));
    let state = DesktopState::open(root.clone()).expect("state");
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
    let previous_credentials = credentials(&account_id, "previous", 10, 1);
    let attempted_credentials = credentials(&account_id, "attempted", 20, 2);
    let newer_credentials = credentials(&account_id, "newer", 30, 3);
    let previous_account = account(&previous_credentials);
    let attempted_account = account(&attempted_credentials);
    let newer_account = account(&newer_credentials);

    credential_store
        .save(&newer_credentials)
        .expect("newer secret");
    state
        .store()
        .expect("store")
        .upsert_account(newer_account.clone())
        .expect("newer account");
    let commit = ImportedAccountCommit {
        account_id: account_id.clone(),
        previous_credentials: Some(previous_credentials),
        previous_account: Some(previous_account),
        attempted_credentials,
        attempted_account,
        runtime_sync_required: false,
    };

    assert!(
        !restore_import_durable_state_if_current(&credential_store, &state, &commit)
            .expect("stale rollback")
    );
    assert!(credential_store
        .require(&account_id)
        .expect("newer secret")
        .matches_snapshot(&newer_credentials));
    assert_eq!(
        state.store().expect("store").account(&account_id),
        Some(&newer_account)
    );

    credential_store
        .delete(&account_id)
        .expect("cleanup account secret");
    drop(state);
    std::fs::remove_dir_all(root).expect("cleanup state");
}

#[tokio::test]
async fn stale_import_reconciliation_persists_the_authoritative_tokens() {
    let root = std::env::temp_dir().join(format!(
        "zenith-relay-import-authority-reconcile-{}",
        uuid::Uuid::new_v4()
    ));
    let state = DesktopState::open(root.clone()).expect("state");
    let credential_store = CredentialStore::from_backend(NativeSecretBackend);
    let account_id = format!("account_{}", uuid::Uuid::new_v4().simple());
    let previous_credentials = credentials(&account_id, "previous", 10, 1);
    let attempted_credentials = credentials(&account_id, "attempted", 20, 2);
    let authoritative_credentials = credentials(&account_id, "authoritative", 30, 3);
    let previous_account = account(&previous_credentials);
    let attempted_account = account(&attempted_credentials);
    credential_store
        .save(&attempted_credentials)
        .expect("attempted secret");
    state
        .store()
        .expect("store")
        .upsert_account(attempted_account.clone())
        .expect("attempted account");

    let authority = state.token_authority();
    authority
        .register(
            &account_id,
            authoritative_credentials
                .to_token_set()
                .expect("authoritative tokens"),
            AccountAuthState::Active,
        )
        .await
        .expect("authority");
    let attempted_tokens = attempted_credentials
        .to_token_set()
        .expect("attempted tokens");
    assert!(!authority
        .register_if_newer(
            &account_id,
            attempted_tokens.clone(),
            AccountAuthState::Active
        )
        .await
        .expect("stale registration"));

    let commit = ImportedAccountCommit {
        account_id: account_id.clone(),
        previous_credentials: Some(previous_credentials),
        previous_account: Some(previous_account),
        attempted_credentials: attempted_credentials.clone(),
        attempted_account,
        runtime_sync_required: false,
    };
    let locks =
        ProcessAccountLocks::with_config(state.transient_root(), ProcessLockConfig::default())
            .expect("locks");
    reconcile_import_authority(
        &state,
        &credential_store,
        &locks,
        &commit,
        &attempted_tokens,
    )
    .await
    .expect("reconcile");

    assert!(credential_store
        .require(&account_id)
        .expect("authoritative secret")
        .matches_snapshot(&authoritative_credentials));
    let account = state
        .store()
        .expect("store")
        .account(&account_id)
        .cloned()
        .expect("account");
    assert_eq!(account.account.token_generation, 3);
    assert_eq!(account.account.token_updated_at_ms, Some(30));
    assert_eq!(account.account.auth_state, AccountAuthState::Active);

    credential_store
        .delete(&account_id)
        .expect("cleanup account secret");
    drop(state);
    std::fs::remove_dir_all(root).expect("cleanup state");
}
