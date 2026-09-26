use super::runtime::*;

#[tokio::test]
async fn manual_auth_recovery_restores_health_but_preserves_disables() {
    let pool = ReviewPool::new().await;
    for id in &pool.ids {
        pool.state
            .account_metadata_sink()
            .persist_auth_state(
                id,
                AccountAuthState::RequiresReauth(ReauthReason::InvalidatedRefreshToken),
            )
            .await
            .unwrap();
    }
    let mut recovered = pool
        .state
        .store()
        .unwrap()
        .account(&pool.ids[0])
        .unwrap()
        .clone();
    recovered.account.auth_state = AccountAuthState::Active;
    recovered.account.last_error_code = None;
    pool.state
        .store()
        .unwrap()
        .upsert_account(recovered.clone())
        .unwrap();
    assert!(sync_account_state_if_running(&pool.state, &pool.ids[0]).await);
    let models = pool.models().await;
    recovered.account.enabled = false;
    pool.state
        .store()
        .unwrap()
        .upsert_account(recovered)
        .unwrap();
    assert!(sync_account_state_if_running(&pool.state, &pool.ids[0]).await);
    let disabled_models = pool.models().await;
    pool.close().await;
    assert_eq!(models, ["gpt-review"]);
    assert!(disabled_models.is_empty());
}
use crate::local_pool::{
    accounts::{
        authority::AccountMetadataSink,
        credentials::{CredentialStore, StoredCodexCredentials},
        records, NativeSecretBackend,
    },
    state::DesktopState,
    store::secret_store,
};
use std::{path::PathBuf, sync::Arc};
use zenith_relay_core::{
    accounts::{AccountAuthMode, AccountAuthState, ReauthReason},
    protocol::OperationalStatus,
    CandidateHealth, GatewayRuntime,
};

struct ReviewPool {
    state: DesktopState,
    runtime: Arc<GatewayRuntime>,
    root: PathBuf,
    ids: [String; 2],
    key_ref: String,
    key: String,
    address: std::net::SocketAddr,
}

#[tokio::test]
async fn membership_scope_preserves_disable_drain_and_removal() {
    let pool = ReviewPool::new().await;
    pool.state
        .account_metadata_sink()
        .persist_auth_state(
            &pool.ids[1],
            AccountAuthState::RequiresReauth(ReauthReason::InvalidatedRefreshToken),
        )
        .await
        .unwrap();
    let original = pool
        .state
        .store()
        .unwrap()
        .account(&pool.ids[0])
        .unwrap()
        .clone();
    for (enabled, draining, in_pool) in [
        (false, false, true),
        (true, true, true),
        (true, false, false),
    ] {
        let mut blocked = original.clone();
        blocked.account.enabled = enabled;
        blocked.account.draining = draining;
        blocked.account.in_pool = in_pool;
        pool.state
            .store()
            .unwrap()
            .upsert_account(blocked.clone())
            .unwrap();
        assert!(apply_account_policy_if_running(&pool.state, &blocked).await);
        assert!(refresh_local_gateway_key_scope_if_running(&pool.state)
            .await
            .unwrap());
        assert!(pool.models().await.is_empty());

        pool.state
            .store()
            .unwrap()
            .upsert_account(original.clone())
            .unwrap();
        assert!(apply_account_policy_if_running(&pool.state, &original).await);
        if !in_pool {
            // Rejoining changes authorization; recovering availability does not.
            assert!(refresh_local_gateway_key_scope_if_running(&pool.state)
                .await
                .unwrap());
        }
        assert_eq!(pool.models().await, ["gpt-review"]);
    }
    pool.close().await;
}

#[tokio::test]
async fn desktop_membership_batch_keeps_the_live_runtime_and_rejects_missing_members() {
    let pool = ReviewPool::new().await;
    let membership = |account_ids: Vec<String>, in_pool| super::pool::PoolMembershipInput {
        account_ids,
        source_ids: Vec::new(),
        in_pool,
    };
    let missing = super::pool::apply_local_pool_membership(
        membership(vec![pool.ids[0].clone(), "missing-member".into()], false),
        &pool.state,
    )
    .await;
    assert!(missing.is_err());
    assert_eq!(pool.models().await, ["gpt-review"]);
    assert!(
        pool.state
            .store()
            .unwrap()
            .account(&pool.ids[0])
            .unwrap()
            .account
            .in_pool
    );

    let (removed, hot, _) =
        super::pool::apply_local_pool_membership(membership(pool.ids.to_vec(), false), &pool.state)
            .await
            .unwrap();
    assert!(hot);
    assert!(removed
        .accounts
        .iter()
        .all(|account| !account.account.in_pool));
    assert!(pool.models().await.is_empty());
    assert!(Arc::ptr_eq(
        &pool.runtime,
        &pool.state.gateway.runtime().await.unwrap()
    ));

    let (joined, hot, refresh_ids) = super::pool::apply_local_pool_membership(
        membership(vec![pool.ids[0].clone()], true),
        &pool.state,
    )
    .await
    .unwrap();
    assert!(hot);
    assert_eq!(refresh_ids, vec![pool.ids[0].clone()]);
    assert!(joined
        .accounts
        .iter()
        .any(|account| account.account.id == pool.ids[0] && account.account.in_pool));
    assert_eq!(pool.models().await, ["gpt-review"]);
    assert!(Arc::ptr_eq(
        &pool.runtime,
        &pool.state.gateway.runtime().await.unwrap()
    ));
    pool.close().await;
}

#[tokio::test]
async fn desktop_proxy_fence_targets_inherited_and_bypassed_accounts() {
    let pool = ReviewPool::new().await;
    let credentials = CredentialStore::from_backend(NativeSecretBackend);
    let affected = super::gateway::accounts_without_explicit_proxy(&pool.state, false).unwrap();
    assert_eq!(affected, pool.ids);

    let second = credentials.require(&pool.ids[1]).unwrap();
    let explicit = second
        .clone()
        .with_proxy_route(Some("http://127.0.0.1:8080".into()), false)
        .unwrap();
    credentials.save(&explicit).unwrap();
    assert_eq!(
        super::gateway::accounts_without_explicit_proxy(&pool.state, false).unwrap(),
        [pool.ids[0].clone()]
    );
    assert_eq!(
        super::gateway::accounts_without_explicit_proxy(&pool.state, true).unwrap(),
        [pool.ids[0].clone()]
    );

    let bypassed = second.with_proxy_route(None, true).unwrap();
    credentials.save(&bypassed).unwrap();
    assert_eq!(
        super::gateway::accounts_without_explicit_proxy(&pool.state, false).unwrap(),
        [pool.ids[0].clone()]
    );
    assert_eq!(
        super::gateway::accounts_without_explicit_proxy(&pool.state, true).unwrap(),
        pool.ids
    );
    pool.close().await;
}

impl ReviewPool {
    async fn new() -> Self {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let root = std::env::temp_dir().join(format!("relay-rotation-review-{unique}"));
        let state = DesktopState::open(root.clone()).unwrap();
        let ids = [format!("review-a-{unique}"), format!("review-b-{unique}")];
        let now = current_time_ms();
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        for id in &ids {
            let secret = StoredCodexCredentials::new(
                id,
                "synthetic-access".into(),
                Some("synthetic-refresh".into()),
                None,
                Some(now + 3_600_000),
                now,
                1,
                None,
                Some(id.clone()),
                None,
                None,
                Some("plus".into()),
                false,
            )
            .unwrap();
            credentials.save(&secret).unwrap();
            let mut record = records::new_account_record(
                &secret,
                AccountAuthMode::OAuth,
                vec!["gpt-review".into()],
                0,
                now,
            )
            .unwrap();
            record.account.in_pool = true;
            state.store().unwrap().upsert_account(record).unwrap();
        }
        let runtime = runtime_from_store(&state).await.unwrap();
        let key_ref = state
            .store()
            .unwrap()
            .keys()
            .iter()
            .find(|key| key.system)
            .unwrap()
            .secret_ref
            .clone();
        let key = secret_store::load(&key_ref).unwrap().unwrap();
        let address = state.gateway.start(runtime.clone(), 0).await.unwrap();
        Self {
            state,
            runtime,
            root,
            ids,
            key_ref,
            key,
            address,
        }
    }

    async fn models(&self) -> Vec<String> {
        // The plain public model view is local; no upstream discovery or inference.
        let response = reqwest::Client::new()
            .get(format!("http://{}/v1/models", self.address))
            .bearer_auth(&self.key)
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
        let body: serde_json::Value = response.json().await.unwrap();
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["id"].as_str().unwrap().to_string())
            .collect()
    }

    async fn close(self) {
        self.state.gateway.stop().await;
        let credentials = CredentialStore::from_backend(NativeSecretBackend);
        for id in &self.ids {
            credentials.delete(id).unwrap();
        }
        secret_store::delete(&self.key_ref).unwrap();
        drop(self.runtime);
        drop(self.state);
        assert!(self.root.starts_with(std::env::temp_dir()));
        for attempt in 0..50 {
            match std::fs::remove_dir_all(&self.root) {
                Ok(()) => return,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
                Err(error)
                    if attempt < 49 && matches!(error.raw_os_error(), Some(5 | 32 | 145)) =>
                {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                Err(error) => panic!("test fixture cleanup failed: {error}"),
            }
        }
    }
}

#[tokio::test]
async fn mixed_pool_keeps_healthy_account_visible() {
    let pool = ReviewPool::new().await;
    pool.state
        .account_metadata_sink()
        .persist_auth_state(
            &pool.ids[1],
            AccountAuthState::RequiresReauth(ReauthReason::InvalidatedRefreshToken),
        )
        .await
        .unwrap();
    let models = pool.models().await;
    pool.close().await;
    assert_eq!(models, ["gpt-review"]);
}

#[tokio::test]
async fn recovered_account_remains_authorized_after_scope_refresh() {
    let pool = ReviewPool::new().await;
    // A temporary auth error is excluded while a pool-membership edit refreshes the key.
    pool.state
        .account_metadata_sink()
        .persist_auth_state(&pool.ids[0], AccountAuthState::Error)
        .await
        .unwrap();
    assert!(refresh_local_gateway_key_scope_if_running(&pool.state)
        .await
        .unwrap());

    // A successful background check restores A without changing its model inventory.
    let previous = pool
        .state
        .store()
        .unwrap()
        .account(&pool.ids[0])
        .unwrap()
        .clone();
    let mut recovered = previous.clone();
    recovered.account.auth_state = AccountAuthState::Active;
    pool.state
        .store()
        .unwrap()
        .upsert_account(recovered.clone())
        .unwrap();
    sync_refreshed_account_or_rollback(&pool.state, previous, recovered, false)
        .await
        .unwrap();
    pool.state
        .account_metadata_sink()
        .persist_auth_state(
            &pool.ids[1],
            AccountAuthState::RequiresReauth(ReauthReason::InvalidatedRefreshToken),
        )
        .await
        .unwrap();
    let healthy_available = pool
        .runtime
        .candidate_runtime_order()
        .into_iter()
        .find(|candidate| candidate.candidate_id == pool.ids[0])
        .unwrap()
        .available;
    let before_removal = pool.models().await;

    // This is the same hot update used when the user removes B from the pool.
    let mut removed = pool
        .state
        .store()
        .unwrap()
        .account(&pool.ids[1])
        .unwrap()
        .clone();
    removed.account.in_pool = false;
    pool.state
        .store()
        .unwrap()
        .upsert_account(removed.clone())
        .unwrap();
    assert!(apply_account_policy_if_running(&pool.state, &removed).await);
    assert!(refresh_local_gateway_key_scope_if_running(&pool.state)
        .await
        .unwrap());
    let after_removal = pool.models().await;
    pool.close().await;

    assert!(healthy_available);
    assert_eq!(after_removal, ["gpt-review"]);
    assert_eq!(
        before_removal,
        ["gpt-review"],
        "recovery must not require removing another member"
    );
}

#[tokio::test]
async fn refreshed_models_keep_desktop_runtime_and_live_candidate_block() {
    let pool = ReviewPool::new().await;
    pool.state
        .account_metadata_sink()
        .persist_auth_state(
            &pool.ids[1],
            AccountAuthState::RequiresReauth(ReauthReason::InvalidatedRefreshToken),
        )
        .await
        .unwrap();
    let previous = pool
        .state
        .store()
        .unwrap()
        .account(&pool.ids[0])
        .unwrap()
        .clone();
    let mut updated = previous.clone();
    updated.discovered_models = Some(vec!["gpt-new".into()]);
    pool.state
        .store()
        .unwrap()
        .upsert_account(updated.clone())
        .unwrap();
    assert!(pool
        .runtime
        .set_candidate_cooldown(&pool.ids[0], "*", current_time_ms() + 60_000));
    assert!(pool
        .runtime
        .set_candidate_health(&pool.ids[0], CandidateHealth::Blocked));
    sync_refreshed_account_or_rollback(&pool.state, previous, updated, true)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &pool.runtime,
        &pool.state.gateway.runtime().await.unwrap()
    ));
    assert!(pool.models().await.is_empty());
    assert!(pool.runtime.clear_candidate_cooldown(&pool.ids[0], "*"));
    assert!(
        !pool
            .runtime
            .candidate_runtime_order()
            .into_iter()
            .find(|candidate| candidate.candidate_id == pool.ids[0])
            .unwrap()
            .available
    );
    assert!(pool
        .runtime
        .set_candidate_health(&pool.ids[0], CandidateHealth::Healthy));
    assert_eq!(pool.models().await, ["gpt-new"]);
    assert!(pool
        .runtime
        .set_candidate_health(&pool.ids[0], CandidateHealth::Blocked));
    let unchanged = pool
        .state
        .store()
        .unwrap()
        .account(&pool.ids[0])
        .unwrap()
        .clone();
    sync_refreshed_account_or_rollback(&pool.state, unchanged.clone(), unchanged, false)
        .await
        .unwrap();
    assert!(pool.models().await.is_empty());
    pool.close().await;
}

#[tokio::test]
async fn account_card_reflects_live_cooldown() {
    let pool = ReviewPool::new().await;
    assert!(pool.runtime.set_candidate_cooldown(
        &pool.ids[0],
        "gpt-review",
        current_time_ms() + 60_000
    ));
    let snapshot = super::state::build_local_runtime_state(&pool.state)
        .await
        .unwrap();
    let available = snapshot
        .gateway
        .routing_order
        .iter()
        .find(|candidate| candidate.candidate_id == pool.ids[0])
        .unwrap()
        .available;
    let status = snapshot
        .accounts
        .iter()
        .find(|account| account.id == pool.ids[0])
        .unwrap()
        .operational_status;
    pool.close().await;

    assert!(!available);
    assert_ne!(
        status,
        OperationalStatus::Rotation,
        "card must reflect current routing eligibility"
    );
}

#[tokio::test]
async fn auth_sync_does_not_overwrite_newer_account_disable() {
    let pool = ReviewPool::new().await;
    pool.state.gateway.stop().await;
    // Holding a test-owned port makes GatewayManager::start yield under its lock.
    let occupied = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = occupied.local_addr().unwrap().port();
    let mut starting = Box::pin(pool.state.gateway.start(pool.runtime.clone(), port));
    assert!(futures_util::poll!(&mut starting).is_pending());

    let sink = pool.state.account_metadata_sink();
    let mut persisting = sink.persist_auth_state(&pool.ids[0], AccountAuthState::Active);
    assert!(futures_util::poll!(&mut persisting).is_pending());
    let mut disabled = pool
        .state
        .store()
        .unwrap()
        .account(&pool.ids[0])
        .unwrap()
        .clone();
    disabled.account.enabled = false;
    pool.state
        .store()
        .unwrap()
        .upsert_account(disabled.clone())
        .unwrap();
    assert!(pool.runtime.update_account_policy(
        &pool.ids[0],
        runtime_account_policy(&disabled, current_time_ms())
    ));
    drop(occupied);
    starting.await.unwrap();
    persisting.await.unwrap();
    drop(sink);

    let persisted_enabled = pool
        .state
        .store()
        .unwrap()
        .account(&pool.ids[0])
        .unwrap()
        .account
        .enabled;
    let available = pool
        .runtime
        .candidate_runtime_order()
        .into_iter()
        .find(|candidate| candidate.candidate_id == pool.ids[0])
        .unwrap()
        .available;
    pool.close().await;

    assert!(!persisted_enabled);
    assert!(
        !available,
        "an older auth snapshot must not re-enable a subsequently disabled account"
    );
}
