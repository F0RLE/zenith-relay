use super::super::super::{apply_local_gateway_key_scope, runtime_account_policy};
use super::super::now_ms;
use crate::local_pool::{
    models::{OwnershipOperationKind, OwnershipOperationPhase},
    remote::RemoteTargetRecord,
    state::DesktopState,
};

use std::collections::HashSet;

use zenith_relay_core::accounts::AccountAuthState;

use super::execution::{
    account_auth_can_transfer_to_remote, activate_local_account_with_operation,
    commit_local_ownership_change, new_force_activation_operation, new_move_operation,
};
use super::reconcile::{reconcile_remote_account_ids, reconciled_remote_error};
use super::*;
use zenith_relay_core::protocol::RemoteAccountLocation;

use crate::local_pool::{
    accounts::{
        credentials::{CredentialStore, StoredCodexCredentials},
        records, NativeSecretBackend,
    },
    commands::runtime::runtime_from_store,
    store::secret_store,
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use zenith_relay_core::{accounts::AccountAuthMode, GatewayRuntime};

struct OwnershipPool {
    state: DesktopState,
    root: PathBuf,
    account_id: String,
    key_ref: String,
    key: String,
    address: SocketAddr,
    previous_runtime: Arc<GatewayRuntime>,
}

impl OwnershipPool {
    async fn new() -> Self {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("relay-ownership-fence-{unique}"));
        let account_id = format!("account-{unique}");
        let now = now_ms();
        let state = DesktopState::open(root.clone()).unwrap();
        let credentials = StoredCodexCredentials::new(
            &account_id,
            "synthetic-access".into(),
            Some("synthetic-refresh".into()),
            None,
            Some(now + 3_600_000),
            now,
            1,
            None,
            Some(account_id.clone()),
            None,
            None,
            Some("plus".into()),
            false,
        )
        .unwrap();
        CredentialStore::from_backend(NativeSecretBackend)
            .save(&credentials)
            .unwrap();
        let mut account = records::new_account_record(
            &credentials,
            AccountAuthMode::OAuth,
            vec!["gpt-ownership".into()],
            0,
            now,
        )
        .unwrap();
        account.account.in_pool = true;
        state.store().unwrap().upsert_account(account).unwrap();
        let previous_runtime = runtime_from_store(&state).await.unwrap();
        let key_ref = state.store().unwrap().keys()[0].secret_ref.clone();
        let key = secret_store::load(&key_ref).unwrap().unwrap();
        let address = state
            .gateway
            .start(previous_runtime.clone(), 0)
            .await
            .unwrap();
        let mut gateway = state.store().unwrap().gateway().clone();
        gateway.port = address.port();
        state.store().unwrap().replace_gateway(gateway).unwrap();
        Self {
            state,
            root,
            account_id,
            key_ref,
            key,
            address,
            previous_runtime,
        }
    }

    async fn models(&self) -> Vec<String> {
        let response: serde_json::Value = reqwest::Client::new()
            .get(format!("http://{}/v1/models", self.address))
            .bearer_auth(&self.key)
            .header(reqwest::header::CONNECTION, "close")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        response["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap().to_string())
            .collect()
    }

    async fn close(self) {
        self.state.gateway.stop().await;
        CredentialStore::from_backend(NativeSecretBackend)
            .delete(&self.account_id)
            .unwrap();
        secret_store::delete(&self.key_ref).unwrap();
        drop(self.previous_runtime);
        drop(self.state);
        for attempt in 0..50 {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error)
                    if attempt < 49 && matches!(error.raw_os_error(), Some(5 | 32 | 145)) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Err(error) => {
                    assert!(!self.root.exists(), "test fixture cleanup failed: {error}");
                    return;
                }
            }
        }
    }
}

fn target() -> RemoteTargetRecord {
    RemoteTargetRecord {
        origin: "https://relay.example.invalid".into(),
        server_id: "server-synthetic".into(),
        identity_fingerprint: "synthetic".into(),
        server_version: "1.1.3".into(),
        protocol_version: 1,
        allow_insecure_http: false,
        secret_ref: "remote:synthetic".into(),
        connected_at_ms: now_ms(),
    }
}

#[tokio::test]
async fn moving_local_owner_retires_previous_runtime_and_removes_route() {
    let pool = OwnershipPool::new().await;
    assert_eq!(pool.models().await, ["gpt-ownership"]);
    let mut accounts = pool.state.store().unwrap().accounts().to_vec();
    accounts[0].remote_location = Some(RemoteAccountLocation {
        server_id: target().server_id,
        remote_account_id: "remote-synthetic".into(),
    });
    accounts[0].account.enabled = false;
    accounts[0].account.in_pool = false;
    let mut operation = new_move_operation(&target(), vec![pool.account_id.clone()]);
    operation.phase = OwnershipOperationPhase::MoveLocalCommitted;
    operation.remote_account_ids = vec!["remote-synthetic".into()];
    commit_local_ownership_change(&pool.state, accounts, operation)
        .await
        .unwrap();
    assert!(pool
        .previous_runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    assert!(pool.models().await.is_empty());
    assert!(pool
        .state
        .store()
        .unwrap()
        .account(&pool.account_id)
        .unwrap()
        .remote_location
        .is_some());
    pool.close().await;
}

#[tokio::test]
async fn failed_move_runtime_replacement_keeps_local_route_disabled() {
    let pool = OwnershipPool::new().await;
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut gateway = pool.state.store().unwrap().gateway().clone();
    gateway.port = occupied.local_addr().unwrap().port();
    pool.state
        .store()
        .unwrap()
        .replace_gateway(gateway)
        .unwrap();
    let mut accounts = pool.state.store().unwrap().accounts().to_vec();
    accounts[0].account.enabled = false;
    accounts[0].account.in_pool = false;
    accounts[0].remote_location = Some(RemoteAccountLocation {
        server_id: target().server_id,
        remote_account_id: "remote-synthetic".into(),
    });
    let mut operation = new_move_operation(&target(), vec![pool.account_id.clone()]);
    operation.phase = OwnershipOperationPhase::MoveLocalCommitted;
    operation.remote_account_ids = vec!["remote-synthetic".into()];
    assert!(
        commit_local_ownership_change(&pool.state, accounts, operation)
            .await
            .is_err()
    );
    let saved = pool
        .state
        .store()
        .unwrap()
        .account(&pool.account_id)
        .unwrap()
        .clone();
    assert!(!saved.account.enabled && !saved.account.in_pool);
    assert!(saved.remote_location.is_some());
    assert_eq!(
        pool.state
            .store()
            .unwrap()
            .ownership_operation()
            .unwrap()
            .phase,
        OwnershipOperationPhase::MoveLocalCommitted
    );
    assert_eq!(pool.state.gateway.address().await, Some(pool.address));
    assert!(pool
        .previous_runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    assert!(pool.models().await.is_empty());
    drop(occupied);
    pool.close().await;
}

#[tokio::test]
async fn failed_return_runtime_replacement_restores_remote_ownership() {
    let pool = OwnershipPool::new().await;
    let location = RemoteAccountLocation {
        server_id: target().server_id,
        remote_account_id: "remote-synthetic".into(),
    };
    let mut account = pool
        .state
        .store()
        .unwrap()
        .account(&pool.account_id)
        .unwrap()
        .clone();
    account.account.enabled = false;
    account.account.in_pool = false;
    account.remote_location = Some(location.clone());
    pool.state
        .store()
        .unwrap()
        .upsert_account(account.clone())
        .unwrap();
    assert!(pool
        .previous_runtime
        .update_account_policy(&pool.account_id, runtime_account_policy(&account, now_ms())));
    assert!(apply_local_gateway_key_scope(&pool.state, &pool.previous_runtime).unwrap());
    assert!(pool.models().await.is_empty());

    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut gateway = pool.state.store().unwrap().gateway().clone();
    gateway.port = occupied.local_addr().unwrap().port();
    pool.state
        .store()
        .unwrap()
        .replace_gateway(gateway)
        .unwrap();
    let mut operation = new_force_activation_operation(&location, pool.account_id.clone());
    operation.phase = OwnershipOperationPhase::ForceLocalCommitted;
    assert!(
        activate_local_account_with_operation(&pool.state, &pool.account_id, operation)
            .await
            .is_err()
    );
    let saved = pool
        .state
        .store()
        .unwrap()
        .account(&pool.account_id)
        .unwrap()
        .clone();
    assert!(!saved.account.enabled && !saved.account.in_pool);
    assert_eq!(saved.remote_location, Some(location));
    assert!(pool.state.store().unwrap().ownership_operation().is_none());
    assert_eq!(pool.state.gateway.address().await, Some(pool.address));
    assert!(pool.models().await.is_empty());
    drop(occupied);
    pool.close().await;
}

#[tokio::test]
async fn remote_reconciliation_revokes_an_erroneously_enabled_live_account() {
    let pool = OwnershipPool::new().await;
    let mut account = pool
        .state
        .store()
        .unwrap()
        .account(&pool.account_id)
        .unwrap()
        .clone();
    account.remote_location = Some(RemoteAccountLocation {
        server_id: target().server_id,
        remote_account_id: "remote-synthetic".into(),
    });
    pool.state.store().unwrap().upsert_account(account).unwrap();
    assert_eq!(pool.models().await, ["gpt-ownership"]);

    reconcile_remote_account_ids(&pool.state, &target(), &HashSet::new())
        .await
        .unwrap();
    let reconciled = pool
        .state
        .store()
        .unwrap()
        .account(&pool.account_id)
        .unwrap()
        .clone();
    assert!(!reconciled.account.enabled && !reconciled.account.in_pool);
    assert_eq!(
        reconciled.account.last_error_code.as_deref(),
        Some(REMOTE_MISSING_ERROR)
    );
    assert!(Arc::ptr_eq(
        &pool.previous_runtime,
        &pool.state.gateway.runtime().await.unwrap()
    ));
    assert!(pool.models().await.is_empty());
    pool.close().await;
}

#[tokio::test]
async fn runtime_rebuild_cannot_route_an_unreconciled_remote_account() {
    let pool = OwnershipPool::new().await;
    let mut account = pool
        .state
        .store()
        .unwrap()
        .account(&pool.account_id)
        .unwrap()
        .clone();
    account.remote_location = Some(RemoteAccountLocation {
        server_id: target().server_id,
        remote_account_id: "remote-synthetic".into(),
    });
    pool.state.store().unwrap().upsert_account(account).unwrap();
    let rebuilt = runtime_from_store(&pool.state).await.unwrap();
    pool.state.gateway.stop().await;
    pool.state
        .gateway
        .start(rebuilt, pool.address.port())
        .await
        .unwrap();
    assert!(pool.models().await.is_empty());
    pool.close().await;
}

#[tokio::test]
async fn pending_remote_move_cannot_reopen_a_local_route_on_restart() {
    let pool = OwnershipPool::new().await;
    let operation = new_move_operation(&target(), vec![pool.account_id.clone()]);
    pool.state
        .store()
        .unwrap()
        .replace_ownership_operation(Some(operation))
        .unwrap();
    assert!(apply_local_gateway_key_scope(&pool.state, &pool.previous_runtime).unwrap());
    assert!(pool.models().await.is_empty());
    let rebuilt = runtime_from_store(&pool.state).await.unwrap();
    pool.state.gateway.stop().await;
    pool.state
        .gateway
        .start(rebuilt, pool.address.port())
        .await
        .unwrap();
    assert!(pool.models().await.is_empty());
    pool.close().await;
}

#[tokio::test]
async fn failed_reconciliation_rebuild_does_not_restore_remote_owned_route() {
    let pool = OwnershipPool::new().await;
    let mut account = pool
        .state
        .store()
        .unwrap()
        .account(&pool.account_id)
        .unwrap()
        .clone();
    account.account.id = format!("remote-only-{}", pool.account_id);
    account.remote_location = Some(RemoteAccountLocation {
        server_id: target().server_id,
        remote_account_id: "remote-synthetic".into(),
    });
    let remote_only_id = account.account.id.clone();
    pool.state.store().unwrap().upsert_account(account).unwrap();
    // This new saved account was not part of the old runtime. Hot apply
    // must rebuild; a blocked new port exercises the safe fallback.
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut gateway = pool.state.store().unwrap().gateway().clone();
    gateway.port = occupied.local_addr().unwrap().port();
    pool.state
        .store()
        .unwrap()
        .replace_gateway(gateway)
        .unwrap();
    assert!(
        reconcile_remote_account_ids(&pool.state, &target(), &HashSet::new())
            .await
            .is_err()
    );
    let saved = pool
        .state
        .store()
        .unwrap()
        .account(&remote_only_id)
        .unwrap()
        .clone();
    assert!(!saved.account.enabled && !saved.account.in_pool);
    assert_eq!(pool.state.gateway.address().await, Some(pool.address));
    assert!(pool
        .previous_runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    assert_eq!(pool.models().await, ["gpt-ownership"]);
    drop(occupied);
    pool.close().await;
}

#[test]
fn access_only_account_cannot_start_server_transfer() {
    assert!(account_auth_can_transfer_to_remote(
        AccountAuthState::Active
    ));
    assert!(!account_auth_can_transfer_to_remote(
        AccountAuthState::DegradedAccessOnly
    ));
}

#[test]
fn remote_reconciliation_is_fail_closed_and_clears_only_its_own_error() {
    assert_eq!(
        reconciled_remote_error(None, false).as_deref(),
        Some(REMOTE_MISSING_ERROR)
    );
    assert_eq!(
        reconciled_remote_error(Some(REMOTE_MISSING_ERROR), true),
        None
    );
    assert_eq!(
        reconciled_remote_error(Some("token_invalidated"), true).as_deref(),
        Some("token_invalidated")
    );
}

#[test]
fn forced_local_recovery_is_a_valid_persisted_ownership_operation() {
    let operation = new_force_activation_operation(
        &RemoteAccountLocation {
            server_id: "server-one".into(),
            remote_account_id: "account-remote".into(),
        },
        "account-local".into(),
    );

    assert!(operation.validate().is_ok());
    assert_eq!(operation.kind, OwnershipOperationKind::ForceActivateLocal);
    assert_eq!(operation.phase, OwnershipOperationPhase::ForcePrepared);
}
