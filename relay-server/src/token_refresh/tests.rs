use super::client::collect_token_response;
use super::*;
use crate::test_fixtures::test_app_state;
use tempfile::TempDir;
use zenith_relay_core::accounts::TokenPersistenceAdapter;
use zenith_relay_core::accounts::TokenRefreshAdapter;

#[tokio::test]
async fn refresh_client_never_falls_back_to_direct_for_unknown_account() {
    let clients = ServerRefreshClients {
        direct: CodexRefreshClient::new_with_proxy(None).unwrap(),
        direct_accounts: HashSet::new(),
        clients: HashMap::new(),
    };
    let failure = clients
        .refresh("proxy-required", "unused-refresh-token", 1)
        .await
        .unwrap_err();
    assert_eq!(failure.code, "proxy_client_missing");
}

#[tokio::test]
async fn old_persistence_cannot_write_a_replaced_login_or_auth_state() {
    let root = TempDir::new().unwrap();
    let state = test_app_state(root.path());
    let mut record: ServerAccountRecord = serde_json::from_value(serde_json::json!({
        "id": "synthetic", "label": "Synthetic", "identityHint": "synthetic",
        "enabled": true, "inPool": false, "draining": false,
        "sourceId": "openai_codex", "secretRef": "account:synthetic:old",
        "authState": AccountAuthState::Active, "health": "healthy",
        "models": [], "allowedModels": [], "excludedModels": [], "priority": 0,
        "weight": 1, "subscription": zenith_relay_core::quota::Subscription::default(),
        "quota": zenith_relay_core::quota::QuotaSnapshot::default(),
        "cooldowns": {}, "consecutiveFailures": 0
    }))
    .unwrap();
    let old_record = record.clone();
    let old_persistence = ServerTokenPersistence::for_account(state.clone(), &record);
    record.secret_ref = "account:synthetic:new".into();
    state.store.save_account(&record).unwrap();
    let credential = crate::state::AccountCredential {
        access_token: "synthetic-new-access".into(),
        refresh_token: Some("synthetic-new-refresh".into()),
        id_token: None,
        expires_at_ms: Some(crate::state::now_ms() + 3_600_000),
        issued_at_ms: 10,
        generation: 1,
        chatgpt_account_id: "synthetic-provider-account".into(),
        responses_url: "https://provider.example.test/v1/responses".into(),
        proxy_url: None,
        agent_private_key: None,
        agent_runtime_id: None,
        agent_task_id: None,
    };
    let encoded = serde_json::to_string(&credential).unwrap();
    state.vault.save(&record.secret_ref, &encoded).unwrap();
    let rejected = TokenSet::new(
        "synthetic-old-access",
        Some("synthetic-old-refresh".into()),
        None,
        Some(60_000),
        10,
        2,
    )
    .unwrap();

    assert!(old_persistence
        .persist(&record.id, &rejected)
        .await
        .is_err());
    assert!(old_persistence
        .persist_auth_state(&record.id, AccountAuthState::Refreshing)
        .await
        .is_err());
    assert!(old_persistence
        .persist_agent_task_id(&record.id, None, "synthetic-task")
        .await
        .is_err());
    assert_eq!(
        state.vault.load(&record.secret_ref).unwrap().as_deref(),
        Some(encoded.as_str())
    );
    assert_eq!(
        state.store.account(&record.id).unwrap().unwrap().auth_state,
        AccountAuthState::Active
    );
    assert!(state.prepare_account_tokens(&old_record).await.is_err());
    assert!(state.token_authority.tokens(&record.id).await.is_none());
    assert_eq!(
        state
            .prepare_account_tokens(&record)
            .await
            .unwrap()
            .access_token(),
        "synthetic-new-access"
    );
    // Runtime rebuilds may hold configuration_lock while waiting for the
    // token authority slot. Persistence must use only the credential lock.
    let configuration = state.configuration_lock.lock().await;
    tokio::time::timeout(
        Duration::from_secs(2),
        ServerTokenPersistence::for_account(state.clone(), &record)
            .persist(&record.id, &credential.tokens().unwrap()),
    )
    .await
    .expect("token persistence must not wait for configuration_lock")
    .unwrap();
    drop(configuration);

    // Even when the account and secret reference remain the same, a late
    // task registration belongs to the old Agent Identity, not a new one
    // that also has no task id yet.
    const TEST_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g";
    let old_agent =
        AgentIdentityCredential::unregistered(TEST_KEY.into(), "old-runtime".into()).unwrap();
    let mut replacement = credential.clone();
    replacement.agent_private_key = Some(TEST_KEY.into());
    replacement.agent_runtime_id = Some("new-runtime".into());
    let encoded_replacement = serde_json::to_string(&replacement).unwrap();
    state
        .vault
        .save(&record.secret_ref, &encoded_replacement)
        .unwrap();
    assert!(ServerTokenPersistence::for_account(state.clone(), &record)
        .persist_agent_task_id_for_identity(&record.id, &old_agent, "old-task")
        .await
        .is_err());
    assert_eq!(
        state.vault.load(&record.secret_ref).unwrap().as_deref(),
        Some(encoded_replacement.as_str())
    );
    state.refresh.shutdown().await;
}

#[tokio::test]
async fn oauth_response_body_is_bounded_while_streaming() {
    use axum::{
        body::{Body, Bytes},
        routing::get,
        Router,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let service = Router::new().route(
        "/token",
        get(|| async {
            Body::from_stream(futures_util::stream::iter(vec![
                Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; MAX_TOKEN_RESPONSE_BYTES])),
                Ok(Bytes::from_static(b"overflow")),
            ]))
        }),
    );
    let server = tokio::spawn(async move { axum::serve(listener, service).await.unwrap() });
    let response = reqwest::get(format!("http://{address}/token"))
        .await
        .unwrap();
    let failure = collect_token_response(response).await.unwrap_err();
    assert_eq!(failure.code, "response_too_large");
    server.abort();
}
