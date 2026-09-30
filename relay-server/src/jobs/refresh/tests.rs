use super::*;
use crate::{
    config::Config,
    store::{Store, Vault},
};
use tempfile::TempDir;
use zenith_relay_core::protocol::RefreshStatus;
use zenith_relay_core::quota::{QuotaWindow, QuotaWindowKind};
use zenith_relay_core::scheduler::refresh::RefreshFreshness;
use zenith_relay_core::CandidateHealth;

fn fixture() -> (TempDir, Arc<AppState>) {
    let root = TempDir::new().unwrap();
    let config = Config::for_test(root.path().into(), "127.0.0.1:0".parse().unwrap());
    let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
    (root, AppState::new(config, store, vault).unwrap())
}

#[tokio::test]
async fn changed_models_refresh_keeps_server_runtime_and_its_cooldown() {
    let (_root, state) = fixture();
    let mut record = account(true);
    record.in_pool = true;
    let credential = AccountCredential {
        access_token: "synthetic-access".into(),
        refresh_token: None,
        id_token: None,
        expires_at_ms: None,
        issued_at_ms: 1,
        generation: 0,
        chatgpt_account_id: "synthetic-provider".into(),
        responses_url: "http://127.0.0.1:9/v1/responses".into(),
        proxy_url: None,
        agent_private_key: None,
        agent_runtime_id: None,
        agent_task_id: None,
    };
    state.store.save_account(&record).unwrap();
    state
        .vault
        .save(
            &record.secret_ref,
            &serde_json::to_string(&credential).unwrap(),
        )
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let runtime = state.runtime().unwrap().unwrap();
    let retry_at = now_ms() + 60_000;
    assert!(runtime.set_candidate_cooldown(&record.id, "*", retry_at));
    assert!(runtime.set_candidate_health(&record.id, CandidateHealth::Blocked));

    record.discovered_models = Some(vec!["added".into()]);
    state.store.save_account(&record).unwrap();
    let (_, fence) = state.store.account_refresh_scope(&record.id).unwrap();
    runtime::synchronize(&state, &fence, true, false)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));
    assert_eq!(
        state.snapshot().unwrap().gateway.visible_model_ids,
        ["added"]
    );
    assert!(runtime.clear_candidate_cooldown(&record.id, "*"));
    assert!(
        !runtime
            .candidate_runtime_order()
            .into_iter()
            .find(|candidate| candidate.candidate_id == record.id)
            .unwrap()
            .available,
        "a model read is not evidence to lift a newer live health block"
    );
    assert!(runtime.set_candidate_health(&record.id, CandidateHealth::Healthy));
    assert_eq!(
        state.snapshot().unwrap().gateway.visible_model_ids,
        ["added"]
    );
    assert!(runtime.set_candidate_health(&record.id, CandidateHealth::Blocked));
    runtime::synchronize(&state, &fence, false, false)
        .await
        .unwrap();
    assert!(
        !runtime
            .candidate_runtime_order()
            .into_iter()
            .find(|candidate| candidate.candidate_id == record.id)
            .unwrap()
            .available
    );
    // A durable positive health transition is the separate evidence to reopen.
    runtime::synchronize(&state, &fence, false, true)
        .await
        .unwrap();
    assert!(
        runtime
            .candidate_runtime_order()
            .into_iter()
            .find(|candidate| candidate.candidate_id == record.id)
            .unwrap()
            .available
    );
    state.shutdown_runtime().await.unwrap();
}

fn account(enabled: bool) -> ServerAccountRecord {
    serde_json::from_value(serde_json::json!({
        "id": "synthetic", "label": "Synthetic", "identityHint": "synthetic",
        "enabled": enabled, "inPool": false, "draining": false, "sourceId": "openai_codex",
        "secretRef": "account:synthetic", "authState": zenith_relay_core::accounts::AccountAuthState::Active, "health": "healthy",
        "models": ["test"], "allowedModels": [], "excludedModels": [], "priority": 0,
        "weight": 1, "subscription": zenith_relay_core::quota::Subscription::default(), "quota": zenith_relay_core::quota::QuotaSnapshot::default(), "cooldowns": {}, "consecutiveFailures": 0
    })).unwrap()
}

#[test]
fn server_snapshot_does_not_claim_persisted_quota_or_models_are_fresh_after_restart() {
    let (_root, state) = fixture();
    let mut record = account(false);
    record.quota.updated_at_ms = Some(1);
    state.store.save_account(&record).unwrap();
    let snapshot = state.snapshot().unwrap();
    assert_eq!(
        snapshot.accounts[0].refresh_state.models,
        RefreshStatus::Stale
    );
    assert_eq!(
        snapshot.accounts[0].refresh_state.quota,
        RefreshStatus::Stale
    );
}

#[tokio::test]
async fn manual_quota_and_models_are_separate_reads_and_keep_last_good_inventory() {
    let (_root, state) = fixture();
    state.store.save_account(&account(false)).unwrap();
    // Deliberately no credential: neither read can contact an external provider.
    let updated = refresh_account_now(&state, account(false)).await.unwrap();
    assert!(updated.quota.error.is_some());
    assert_eq!(updated.models, ["test"]);
    assert!(!updated.enabled && !updated.in_pool);
    assert_eq!(
        updated.last_error_code.as_deref(),
        Some("models_secret_missing")
    );
    state.refresh.shutdown().await;
}

#[tokio::test]
async fn disabled_registration_never_polls_but_enabled_out_of_pool_monitoring_does() {
    let (_root, state) = fixture();
    state.store.save_account(&account(false)).unwrap();
    reconcile(&state).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(state
        .store
        .account("synthetic")
        .unwrap()
        .unwrap()
        .quota
        .error
        .is_none());
    state.store.save_account(&account(true)).unwrap();
    reconcile(&state).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        let mut changes = state.store.refresh_changes();
        loop {
            changes.borrow_and_update();
            if state
                .store
                .account("synthetic")
                .unwrap()
                .unwrap()
                .quota
                .error
                .is_some()
            {
                break;
            }
            changes.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(!state.store.account("synthetic").unwrap().unwrap().in_pool);
    state.refresh.shutdown().await;
}

#[tokio::test]
async fn a_replaced_revision_is_rejected_before_provider_preparation() {
    let (_root, state) = fixture();
    state.store.save_account(&account(true)).unwrap();
    let (_, fence) = state.store.account_refresh_scope("synthetic").unwrap();
    state.store.save_account(&account(false)).unwrap();
    let job = RefreshJob {
        id: zenith_relay_core::scheduler::refresh::RefreshJobId(1),
        identity: fence.identity(),
        kind: RefreshKind::Quota,
        due_at_ms: 0,
        manual: true,
    };
    assert!(execute(&state, &fence, &job).await.is_err());
    assert!(state
        .store
        .account("synthetic")
        .unwrap()
        .unwrap()
        .quota
        .error
        .is_none());
    assert_eq!(
        state
            .refresh
            .freshness(&fence.identity(), RefreshKind::Quota),
        RefreshFreshness::Unknown
    );
}

#[tokio::test]
async fn authorization_is_on_demand_transient_and_rejects_replaced_revision() {
    let (_root, state) = fixture();
    let record = account(false);
    state.store.save_account(&record).unwrap();
    let credential = crate::state::AccountCredential {
        access_token: "synthetic-access".into(),
        refresh_token: None,
        id_token: None,
        expires_at_ms: Some(now_ms() + 3_600_000),
        issued_at_ms: now_ms(),
        generation: 1,
        chatgpt_account_id: "synthetic-provider-account".into(),
        responses_url: "https://provider.example.test/v1/responses".into(),
        proxy_url: None,
        agent_private_key: None,
        agent_runtime_id: None,
        agent_task_id: None,
    };
    state
        .vault
        .save(
            &record.secret_ref,
            &serde_json::to_string(&credential).unwrap(),
        )
        .unwrap();
    let (_, fence) = state.store.account_refresh_scope(&record.id).unwrap();
    register(
        &state,
        &record,
        fence.clone(),
        RefreshKind::Auth,
        false,
        false,
    )
    .unwrap();
    assert_eq!(
        state
            .refresh
            .freshness(&fence.identity(), RefreshKind::Auth),
        RefreshFreshness::Unknown
    );
    let prepared = request_authorization(&state, &fence).await.unwrap();
    assert!(prepared.header.is_sensitive());
    assert_eq!(
        prepared
            .oauth_tokens
            .as_ref()
            .map(|tokens| tokens.generation()),
        Some(1)
    );
    assert!(state
        .refresh
        .cached(&fence.identity(), RefreshKind::Auth)
        .is_none());

    let mut changed = record;
    changed.enabled = true;
    state.store.save_account(&changed).unwrap();
    assert!(matches!(
        prepare_authorization(&state, &fence).await,
        Err(AuthorizationFailure::Stale)
    ));
    state.refresh.shutdown().await;
}

#[tokio::test]
async fn old_oauth_rejection_cannot_invalidate_a_newer_server_generation() {
    use zenith_relay_core::accounts::TokenSet;

    let (_root, state) = fixture();
    let record = account(false);
    state.store.save_account(&record).unwrap();
    let current = crate::state::AccountCredential {
        access_token: "synthetic-new-access".into(),
        refresh_token: Some("synthetic-refresh".into()),
        id_token: None,
        expires_at_ms: Some(now_ms() + 3_600_000),
        issued_at_ms: now_ms(),
        generation: 8,
        chatgpt_account_id: "synthetic-provider-account".into(),
        responses_url: "https://provider.example.test/v1/responses".into(),
        proxy_url: None,
        agent_private_key: None,
        agent_runtime_id: None,
        agent_task_id: None,
    };
    state
        .vault
        .save(
            &record.secret_ref,
            &serde_json::to_string(&current).unwrap(),
        )
        .unwrap();
    state
        .token_authority
        .register(
            &record.id,
            TokenSet::new(
                current.access_token.clone(),
                current.refresh_token.clone(),
                None,
                current.expires_at_ms,
                current.issued_at_ms,
                current.generation,
            )
            .unwrap(),
            record.auth_state,
        )
        .await
        .unwrap();

    let recovery = state
        .recover_account_tokens_after_unauthorized(
            &record,
            &TokenSet::new(
                "synthetic-old-access",
                current.refresh_token.clone(),
                None,
                current.expires_at_ms,
                current.issued_at_ms,
                7,
            )
            .unwrap(),
        )
        .await;
    assert!(recovery.is_err());
    let persisted: crate::state::AccountCredential =
        serde_json::from_str(&state.vault.load(&record.secret_ref).unwrap().unwrap()).unwrap();
    assert_eq!(persisted.generation, 8);
    assert_eq!(persisted.expires_at_ms, current.expires_at_ms);
    state.refresh.shutdown().await;
}

#[tokio::test]
async fn quota_and_unchanged_model_reads_keep_the_live_scheduler() {
    let (_root, state) = fixture();
    let mut account = account(true);
    account.in_pool = true;
    state.store.save_account(&account).unwrap();
    let credential = crate::state::AccountCredential {
        access_token: "synthetic-access".into(),
        refresh_token: None,
        id_token: None,
        expires_at_ms: None,
        issued_at_ms: 0,
        generation: 1,
        chatgpt_account_id: "synthetic-provider-account".into(),
        responses_url: "https://provider.example.test/v1/responses".into(),
        proxy_url: None,
        agent_private_key: None,
        agent_runtime_id: None,
        agent_task_id: None,
    };
    state
        .vault
        .save(
            &account.secret_ref,
            &serde_json::to_string(&credential).unwrap(),
        )
        .unwrap();
    // A runtime rebuild must not roll back a token refreshed after it took
    // its stored credential snapshot.
    state
        .token_authority
        .register(
            &account.id,
            zenith_relay_core::accounts::TokenSet::new(
                "synthetic-new-access",
                None,
                None,
                Some(now_ms() + 3_600_000),
                now_ms(),
                2,
            )
            .unwrap(),
            account.auth_state,
        )
        .await
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    assert_eq!(
        state
            .token_authority
            .tokens(&account.id)
            .await
            .unwrap()
            .access_token(),
        "synthetic-new-access"
    );
    let before = state.runtime().unwrap().unwrap();
    let (_, fence) = state.store.account_refresh_scope(&account.id).unwrap();
    state
        .store
        .apply_account_refresh(&fence, |account| {
            account.quota.updated_at_ms = Some(now_ms());
            Ok(())
        })
        .unwrap();
    runtime::synchronize(&state, &fence, false, false)
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&before, &state.runtime().unwrap().unwrap()));
    state
        .store
        .update_account(&account.id, |account| {
            account.enabled = false;
            Ok(())
        })
        .unwrap();
    assert!(runtime::synchronize(&state, &fence, false, false)
        .await
        .is_err());
    assert!(Arc::ptr_eq(&before, &state.runtime().unwrap().unwrap()));
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn server_reset_due_accelerates_only_the_registered_quota_job() {
    let (_root, state) = fixture();
    let account = account(true);
    state.store.save_account(&account).unwrap();
    let (_, fence) = state.store.account_refresh_scope(&account.id).unwrap();
    register(
        &state,
        &account,
        fence.clone(),
        RefreshKind::Quota,
        false,
        false,
    )
    .unwrap();
    register(
        &state,
        &account,
        fence.clone(),
        RefreshKind::Models,
        false,
        false,
    )
    .unwrap();

    let mut updated = account;
    updated.quota.primary = Some(QuotaWindow {
        kind: QuotaWindowKind::Primary,
        provider_cycle_id: None,
        window_start_ms: None,
        available_basis_points: Some(0),
        explicitly_full: Some(false),
        reset_at_ms: Some(now_ms() + 1_000),
        window_minutes: None,
        observed_at_ms: now_ms(),
        full_transition_fingerprint: None,
        exhaustion_transition_fingerprint: None,
    });
    schedule_quota_reset(
        &state,
        &fence,
        &AccountRead {
            account: updated,
            transitions: Vec::new(),
            succeeded: true,
            retry_after_ms: None,
            models_changed: false,
            health_changed: false,
        },
    );
    tokio::time::advance(std::time::Duration::from_secs(25)).await;
    tokio::task::yield_now().await;
    // The registered read has no vault secret; only quota should have run.
    assert!(state
        .store
        .account("synthetic")
        .unwrap()
        .unwrap()
        .quota
        .error
        .is_some());
    assert_ne!(
        state
            .store
            .account("synthetic")
            .unwrap()
            .unwrap()
            .last_error_code
            .as_deref(),
        Some("models_secret_missing")
    );
    state.refresh.shutdown().await;
}
