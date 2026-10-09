use super::*;
use crate::test_fixtures::{pooled_source, test_app_state};
use std::collections::BTreeMap;
use tempfile::TempDir;
use zenith_relay_core::{
    accounts::{AccountAuthState, AccountHealthState},
    quota::{QuotaSnapshot, Subscription},
};

fn test_state(root: &TempDir) -> Arc<AppState> {
    test_app_state(root.path())
}

fn test_account(id: &str) -> ServerAccountRecord {
    ServerAccountRecord {
        id: id.into(),
        label: "Synthetic account".into(),
        identity_hint: "synthetic-hint".into(),
        enabled: true,
        in_pool: true,
        draining: false,
        source_id: "openai_codex".into(),
        secret_ref: format!("account:{id}"),
        provider_family: Some("openai".into()),
        auth_state: AccountAuthState::Active,
        health: AccountHealthState::Healthy,
        models: vec!["gpt-test".into()],
        discovered_models: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        subscription: Subscription::default(),
        quota: QuotaSnapshot::default(),
        purchase_cost_micro_usd: None,
        cooldowns: BTreeMap::new(),
        consecutive_failures: 0,
        created_at_ms: 1,
        last_used_at_ms: None,
        last_error_code: None,
        proxy_id: None,
        bypass_common_proxy: false,
    }
}

fn test_credential() -> AccountCredential {
    AccountCredential {
        oauth_client_kind: Default::default(),
        chatgpt_user_id: None,
        basis_points_headers: None,
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
    }
}

#[test]
fn stored_credentials_reject_mismatched_issuing_client_before_use() {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use zenith_relay_core::providers::chatgpt::OAuthClientKind;
    let mut credential = test_credential();
    let payload = serde_json::json!({"client_id": OAuthClientKind::ExcelBps.client_id()});
    credential.access_token = format!(
        "synthetic.{}.synthetic",
        URL_SAFE_NO_PAD.encode(payload.to_string()),
    );
    assert!(credential.tokens().is_err());
    assert!(credential.agent_identity().is_err());
    credential.oauth_client_kind = OAuthClientKind::ExcelBps;
    assert!(credential.tokens().is_ok());
    credential.agent_runtime_id = Some("synthetic-agent".into());
    assert!(credential.tokens().is_err());
}

#[tokio::test]
async fn failed_vault_delete_restores_account_and_retires_old_runtime() {
    let root = TempDir::new().unwrap();
    let state = test_state(&root);
    let record = test_account("delete-rollback");
    let secret = serde_json::to_string(&test_credential()).unwrap();
    state.store.save_account(&record).unwrap();
    state.vault.save(&record.secret_ref, &secret).unwrap();
    state.rebuild_runtime().await.unwrap();
    let previous = state.runtime().unwrap().unwrap();
    assert!(previous
        .candidate_runtime_order()
        .iter()
        .any(|candidate| candidate.candidate_id == record.id && candidate.available));

    // Force the vault's atomic replace to fail before it writes anything.
    // This is synthetic filesystem state, never a real credential.
    let backup = root.path().join("vault/secrets.enc.bak");
    if backup.is_file() {
        std::fs::remove_file(&backup).unwrap();
    }
    std::fs::create_dir(&backup).unwrap();
    assert!(
        delete_account(State(state.clone()), Path(record.id.clone()))
            .await
            .is_err()
    );
    assert_eq!(
        state.store.account(&record.id).unwrap().unwrap().secret_ref,
        record.secret_ref
    );
    assert_eq!(
        state.vault.load(&record.secret_ref).unwrap().as_deref(),
        Some(secret.as_str())
    );
    let restored = state.runtime().unwrap().unwrap();
    assert!(!Arc::ptr_eq(&previous, &restored));
    assert!(previous
        .candidate_runtime_order()
        .iter()
        .all(|candidate| candidate.candidate_id != record.id || !candidate.available));
    assert!(restored
        .candidate_runtime_order()
        .iter()
        .any(|candidate| candidate.candidate_id == record.id && candidate.available));
    std::fs::remove_dir(&backup).unwrap();
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn account_disable_updates_the_live_runtime_without_a_replacement() {
    let root = TempDir::new().unwrap();
    let state = test_state(&root);
    let record = test_account("synthetic-account");
    let credential = test_credential();
    let mut weighting = record.clone();
    weighting.weight = 3;
    weighting.priority = 2;
    assert!(!account_dispatch_permission_changed(&record, &weighting));
    let mut removed = record.clone();
    removed.in_pool = false;
    assert!(account_dispatch_permission_changed(&record, &removed));
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
    assert!(runtime
        .candidate_runtime_order()
        .iter()
        .any(|candidate| candidate.available));

    let Json(summary) = update_account(
        State(state.clone()),
        Path(record.id.clone()),
        Json(AccountPatch {
            enabled: Some(false),
            ..Default::default()
        }),
    )
    .await
    .unwrap();
    assert!(!summary.enabled);
    assert!(!state.store.account(&record.id).unwrap().unwrap().enabled);
    assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));
    assert!(runtime
        .candidate_runtime_order()
        .iter()
        .all(|candidate| !candidate.available));
    state.shutdown_runtime().await.unwrap();
}

#[tokio::test]
async fn mixed_membership_batch_updates_scopes_without_replacing_the_runtime() {
    let root = TempDir::new().unwrap();
    let state = test_state(&root);
    let account = test_account("batch-account");
    let source = pooled_source("batch-source", "gpt-test");
    state.store.save_account(&account).unwrap();
    state
        .vault
        .save(
            &account.secret_ref,
            &serde_json::to_string(&test_credential()).unwrap(),
        )
        .unwrap();
    state.store.save_source(&source).unwrap();
    state
        .vault
        .save(&source.secret_ref, "synthetic-source-key")
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let runtime = state.runtime().unwrap().unwrap();
    let has_next_candidate = || {
        runtime
            .candidate_runtime_order_for_key(crate::state::SYSTEM_GATEWAY_KEY_ID)
            .into_iter()
            .any(|candidate| candidate.next_for_new_request)
    };
    assert!(has_next_candidate());

    // Validation must happen before any candidate is fenced or any durable
    // member is changed, even when another id in the same batch exists.
    let missing = set_pool_membership(
        State(state.clone()),
        Json(PoolMembershipInput {
            account_ids: vec![account.id.clone()],
            source_ids: vec!["missing-source".into()],
            in_pool: false,
        }),
    )
    .await;
    assert!(missing.is_err());
    assert!(has_next_candidate());
    assert!(state.store.account(&account.id).unwrap().unwrap().in_pool);

    let membership = |in_pool| PoolMembershipInput {
        account_ids: vec![account.id.clone()],
        source_ids: vec![source.id.clone()],
        in_pool,
    };
    let Json(removed) = set_pool_membership(State(state.clone()), Json(membership(false)))
        .await
        .unwrap();
    assert!(removed.accounts.iter().all(|account| !account.in_pool));
    assert!(removed.sources.iter().all(|source| !source.in_pool));
    assert!(!has_next_candidate());
    assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));

    let Json(joined) = set_pool_membership(State(state.clone()), Json(membership(true)))
        .await
        .unwrap();
    assert!(joined.accounts.iter().all(|account| account.in_pool));
    assert!(joined.sources.iter().all(|source| source.in_pool));
    assert!(has_next_candidate());
    assert!(Arc::ptr_eq(&runtime, &state.runtime().unwrap().unwrap()));
    state.shutdown_runtime().await.unwrap();
}
