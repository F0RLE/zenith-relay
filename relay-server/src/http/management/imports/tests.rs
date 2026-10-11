use super::batch::{parse_batch_import_input, BatchImportPreviewInput};

#[tokio::test]
async fn free_bps_preview_is_rejected_before_authentication_or_pending_storage() {
    use super::preview::{prepare_account_import, AccountImportInput};
    use zenith_relay_core::error_codes;
    let root = tempfile::TempDir::new().unwrap();
    let state = crate::test_fixtures::test_app_state(root.path());
    let input: AccountImportInput = serde_json::from_value(serde_json::json!({
        "label":"Synthetic Free BPS",
        "oauthClientKind":"excel_bps",
        "accessToken":"synthetic-access",
        "planType":"chatgpt_free_tier"
    }))
    .unwrap();
    let error = match prepare_account_import(&state, input, None).await {
        Ok(_) => panic!("Free BPS import was accepted"),
        Err(error) => error,
    };
    assert_eq!(
        error.code,
        error_codes::BASIS_POINTS_FREE_ACCOUNT_UNSUPPORTED
    );
    assert!(!error.retryable);
    assert!(state.store.accounts().unwrap().is_empty());
}

#[tokio::test]
async fn authenticated_free_plan_blocks_bps_and_allows_native_pool_import() {
    use super::confirm::confirm_one_account_import;
    use super::preview::{prepare_account_import, AccountImportInput};
    use crate::config::Config;
    use crate::state::AppState;
    use crate::store::{Store, Vault};
    use axum::{routing::get, Json, Router};
    use std::sync::Arc;
    use zenith_relay_core::error_codes;

    let root = tempfile::TempDir::new().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new().route(
        "/accounts/check",
        get(|| async {
            Json(serde_json::json!({
                "accounts": [{
                    "account": {"id": "synthetic-free", "plan_type": "free"}
                }]
            }))
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut config = Config::for_test(root.path().to_path_buf(), "127.0.0.1:0".parse().unwrap());
    config.account_check_url =
        url::Url::parse(&format!("http://{address}/accounts/check")).unwrap();
    let store = Arc::new(Store::open(root.path().join("relay.sqlite")).unwrap());
    let vault = Arc::new(Vault::open(&root.path().join("vault"), config.vault_key).unwrap());
    let state = AppState::new(config, store, vault).unwrap();
    for hinted_plan in [None, Some("plus")] {
        let input: AccountImportInput = serde_json::from_value(serde_json::json!({
            "label": "Synthetic BPS",
            "oauthClientKind": "excel_bps",
            "accessToken": "synthetic-access",
            "chatgptAccountId": "synthetic-free",
            "planType": hinted_plan
        }))
        .unwrap();
        let error = match prepare_account_import(&state, input, None).await {
            Ok(_) => panic!("authenticated Free BPS account was accepted"),
            Err(error) => error,
        };
        assert_eq!(
            error.code,
            error_codes::BASIS_POINTS_FREE_ACCOUNT_UNSUPPORTED
        );
        assert!(state.store.accounts().unwrap().is_empty());
    }
    let input: AccountImportInput = serde_json::from_value(serde_json::json!({
        "label": "Synthetic native",
        "oauthClientKind": "codex",
        "accessToken": "synthetic-native",
        "chatgptAccountId": "synthetic-free",
        "planType": "plus",
        "models": ["gpt-test"]
    }))
    .unwrap();
    let preview = prepare_account_import(&state, input, None).await.unwrap();
    assert_eq!(preview.plan_type.as_deref(), Some("free"));
    let confirmed = confirm_one_account_import(&state, &preview.session_id, None, true, false)
        .await
        .unwrap();
    assert!(confirmed.account.in_pool);
    assert_eq!(
        confirmed.account.subscription.plan_type.as_deref(),
        Some("free")
    );
    state.shutdown_runtime().await.unwrap();
    state.refresh.shutdown().await;
    server.abort();
}

#[tokio::test]
async fn import_and_delete_wait_for_old_build_before_changing_account_incarnation() {
    use super::confirm::confirm_one_account_import;
    use super::preview::AccountImportPreview;
    use crate::{
        state::{now_ms, AccountCredential, ServerAccountRecord},
        store::PendingImport,
    };
    use std::{sync::Arc, time::Duration};
    use tempfile::TempDir;
    use zenith_relay_core::accounts::{AccountAuthState, TokenSet};

    let root = TempDir::new().unwrap();
    let state = crate::test_fixtures::test_app_state(root.path());
    let existing_account: ServerAccountRecord = serde_json::from_value(serde_json::json!({
        "id": "synthetic", "label": "Synthetic", "identityHint": "synthetic",
        "enabled": true, "inPool": true, "draining": false,
        "sourceId": "openai_codex", "secretRef": "account:synthetic:old",
        "authState": AccountAuthState::Active, "health": "healthy", "models": ["test"],
        "allowedModels": [], "excludedModels": [], "priority": 0, "weight": 1,
        "subscription": zenith_relay_core::quota::Subscription::default(),
        "quota": zenith_relay_core::quota::QuotaSnapshot::default(),
        "cooldowns": {}, "consecutiveFailures": 0
    }))
    .unwrap();
    let credential = |access_token: &str, generation| AccountCredential {
        oauth_client_kind: Default::default(),
        chatgpt_user_id: None,
        basis_points_headers: None,
        access_token: access_token.into(),
        refresh_token: None,
        id_token: None,
        expires_at_ms: Some(now_ms() + 3_600_000),
        issued_at_ms: now_ms(),
        generation,
        chatgpt_account_id: "synthetic-provider-account".into(),
        responses_url: "https://provider.example.test/v1/responses".into(),
        proxy_url: None,
        agent_private_key: None,
        agent_runtime_id: None,
        agent_task_id: None,
    };
    state.store.save_account(&existing_account).unwrap();
    state
        .vault
        .save(
            &existing_account.secret_ref,
            &serde_json::to_string(&credential("old-access", 7)).unwrap(),
        )
        .unwrap();
    state.rebuild_runtime().await.unwrap();
    let existing_runtime = state.runtime().unwrap().unwrap();
    assert_eq!(
        state
            .token_authority
            .tokens(&existing_account.id)
            .await
            .unwrap()
            .generation(),
        7
    );

    let session_id = format!("import_{}", uuid::Uuid::new_v4().simple());
    let new_ref = "account:synthetic:new";
    let preview = AccountImportPreview {
        session_id: session_id.clone(),
        oauth_client_kind: Default::default(),
        account_id: existing_account.id.clone(),
        duplicate_account_id: Some(existing_account.id.clone()),
        label: existing_account.label.clone(),
        identity_hint: existing_account.identity_hint.clone(),
        models: existing_account.models.clone(),
        auth_state: AccountAuthState::Active,
        expires_at_ms: None,
        plan_type: None,
        subscription_active_until_ms: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        batch_session_id: None,
    };
    state
        .vault
        .save(
            new_ref,
            &serde_json::to_string(&credential("new-access", 0)).unwrap(),
        )
        .unwrap();
    state
        .store
        .save_pending_import(&PendingImport {
            id: session_id.clone(),
            preview_json: serde_json::to_string(&preview).unwrap(),
            secret_ref: new_ref.into(),
            created_at_ms: now_ms(),
        })
        .unwrap();

    let previous_build = state.lock_runtime_rebuild().await;
    let worker_state = state.clone();
    let worker = tokio::spawn(async move {
        confirm_one_account_import(&worker_state, &session_id, None, false, false).await
    });
    tokio::task::yield_now().await;
    assert_eq!(
        state
            .store
            .account(&existing_account.id)
            .unwrap()
            .unwrap()
            .secret_ref,
        existing_account.secret_ref
    );
    assert!(!worker.is_finished());
    // Complete a build using the old snapshot while the import is queued.
    // Its publication must precede, not follow, the new login's commit.
    previous_build.rebuild(&state).await.unwrap();
    assert_eq!(
        state
            .token_authority
            .tokens(&existing_account.id)
            .await
            .unwrap()
            .generation(),
        7
    );
    drop(previous_build);

    let confirmed = tokio::time::timeout(Duration::from_secs(5), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(confirmed.account.secret_ref, new_ref);
    assert_eq!(
        state
            .store
            .account(&existing_account.id)
            .unwrap()
            .unwrap()
            .secret_ref,
        new_ref
    );
    assert!(state
        .vault
        .load(&existing_account.secret_ref)
        .unwrap()
        .is_none());
    assert!(!Arc::ptr_eq(
        &existing_runtime,
        &state.runtime().unwrap().unwrap()
    ));
    let current: TokenSet = state
        .token_authority
        .tokens(&existing_account.id)
        .await
        .unwrap();
    // A replacement login fences refreshes from the previous generation.
    assert_eq!(current.generation(), 8);
    assert_eq!(current.access_token(), "new-access");

    let active_build = state.lock_runtime_rebuild().await;
    let delete_state = state.clone();
    let account_id = existing_account.id.clone();
    let deletion = tokio::spawn(async move {
        crate::http::management::accounts::delete_account(
            axum::extract::State(delete_state),
            axum::extract::Path(account_id),
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(state.store.account(&existing_account.id).unwrap().is_some());
    assert!(!deletion.is_finished());
    drop(active_build);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), deletion)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        axum::http::StatusCode::NO_CONTENT
    );
    assert!(state.store.account(&existing_account.id).unwrap().is_none());
    assert!(state
        .token_authority
        .tokens(&existing_account.id)
        .await
        .is_none());
    assert!(state.runtime().unwrap().is_none());
    state.shutdown_runtime().await.unwrap();
    state.refresh.shutdown().await;
}

#[test]
fn batch_import_parses_raw_token_lines() {
    let parsed = parse_batch_import_input(BatchImportPreviewInput {
        content: Some("Bearer header.payload.signature\nat-opaque-token".into()),
        documents: Vec::new(),
    })
    .unwrap();

    assert_eq!(parsed.items.len(), 2);
    assert_eq!(
        parsed.items[0].secrets().access_token(),
        Some("header.payload.signature")
    );
    assert_eq!(
        parsed.items[1].secrets().access_token(),
        Some("at-opaque-token")
    );
}

#[tokio::test]
async fn trash_blocks_reimport_under_a_new_id_without_replacing_retained_credentials() {
    use super::confirm::confirm_one_account_import;
    use super::preview::AccountImportPreview;
    use crate::state::{now_ms, AccountCredential};
    use crate::store::PendingImport;

    let root = tempfile::TempDir::new().unwrap();
    let state = crate::test_fixtures::test_app_state(root.path());
    let account = crate::test_fixtures::synthetic_server_account("synthetic-trashed");
    let credential: AccountCredential = serde_json::from_value(serde_json::json!({
        "accessToken": "synthetic-retained-access",
        "chatgptAccountId": "synthetic-provider-identity",
        "responsesUrl": "https://provider.example.test/v1/responses",
        "issuedAtMs": now_ms(),
        "generation": 7
    }))
    .unwrap();
    let retained_secret = serde_json::to_string(&credential).unwrap();
    state
        .vault
        .save(&account.secret_ref, &retained_secret)
        .unwrap();
    state.store.save_account(&account).unwrap();
    state.store.trash_account(&account.id, now_ms()).unwrap();

    let session_id = format!("import_{}", uuid::Uuid::new_v4().simple());
    let preview = AccountImportPreview {
        session_id: session_id.clone(),
        oauth_client_kind: Default::default(),
        account_id: "synthetic-new-id".into(),
        duplicate_account_id: None,
        label: "Synthetic replacement".into(),
        identity_hint: "Synthetic".into(),
        models: account.models.clone(),
        auth_state: account.auth_state,
        expires_at_ms: None,
        plan_type: None,
        subscription_active_until_ms: None,
        allowed_models: Vec::new(),
        excluded_models: Vec::new(),
        priority: 0,
        weight: 1,
        batch_session_id: None,
    };
    let pending_secret_ref = "account:synthetic-new-id:pending";
    let pending_credential = AccountCredential {
        access_token: "synthetic-replacement-access".into(),
        ..credential
    };
    let pending_secret = serde_json::to_string(&pending_credential).unwrap();
    state
        .vault
        .save(pending_secret_ref, &pending_secret)
        .unwrap();
    state
        .store
        .save_pending_import(&PendingImport {
            id: session_id.clone(),
            preview_json: serde_json::to_string(&preview).unwrap(),
            secret_ref: pending_secret_ref.into(),
            created_at_ms: now_ms(),
        })
        .unwrap();

    let error = match confirm_one_account_import(&state, &session_id, None, true, false).await {
        Ok(_) => panic!("trashed account was reimported under a new id"),
        Err(error) => error,
    };
    assert_eq!(
        error.message,
        "account is in trash; restore it before importing"
    );
    assert!(state.store.accounts().unwrap().is_empty());
    assert_eq!(state.store.account_trash().unwrap().len(), 1);
    assert_eq!(
        state.vault.load(&account.secret_ref).unwrap().as_deref(),
        Some(retained_secret.as_str())
    );
    assert_eq!(
        state.vault.load(pending_secret_ref).unwrap().as_deref(),
        Some(pending_secret.as_str())
    );
    assert!(state.store.pending_import(&session_id).unwrap().is_some());
    state.shutdown_runtime().await.unwrap();
    state.refresh.shutdown().await;
}

#[test]
fn batch_import_keeps_valid_documents_when_one_is_malformed() {
    let parsed = parse_batch_import_input(BatchImportPreviewInput {
        content: None,
        documents: vec![
            r#"{"account_id":"account-one","access_token":"access-one"}"#.into(),
            r#"{"access_token":"truncated""#.into(),
        ],
    })
    .unwrap();

    assert_eq!(parsed.items.len(), 1);
    assert_eq!(parsed.preview.rows.len(), 2);
    assert!(parsed.preview.rows[0].error.is_none());
    assert_eq!(
        parsed.preview.rows[1].error.as_ref().unwrap().code,
        zenith_relay_core::accounts::ImportIssueCode::MalformedJson
    );
}

#[test]
fn batch_import_accepts_sub2api_agent_identity() {
    let input = serde_json::json!({
        "name": "Agent account",
        "credentials": {
            "auth_mode": "agentIdentity",
            "agent_private_key": "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g",
            "agent_runtime_id": "runtime-test",
            "task_id": "task-test",
            "chatgpt_account_id": "account-test"
        }
    });
    let parsed = parse_batch_import_input(BatchImportPreviewInput {
        content: Some(input.to_string()),
        documents: Vec::new(),
    })
    .unwrap();
    let normalized = &parsed.items[0];

    assert!(normalized.secrets().access_token().is_none());
    assert_eq!(
        normalized.secrets().agent_runtime_id(),
        Some("runtime-test")
    );
    assert_eq!(normalized.secrets().agent_task_id(), Some("task-test"));
    assert!(
        zenith_relay_core::providers::chatgpt::AgentIdentityCredential::new(
            normalized
                .secrets()
                .agent_private_key()
                .unwrap()
                .to_string(),
            normalized.secrets().agent_runtime_id().unwrap().to_string(),
            normalized.secrets().agent_task_id().unwrap().to_string(),
        )
        .is_ok()
    );
}
